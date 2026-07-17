use std::env;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use gareji_core::board::{BoardActiveWorkAssessment, BoardPort, BoardPortError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const BOARD_BRIDGE_PROTOCOL_VERSION: &str = "gareji.board-bridge.v0";
const MAX_BRIDGE_MESSAGE_BYTES: usize = 1_048_576;

/// Production Adapter for the separately released Gareji Board binary.
pub struct ProcessBoardAdapter {
    command: PathBuf,
    database: Option<PathBuf>,
    process: Mutex<Option<BoardProcess>>,
    request_counter: AtomicU64,
}

impl ProcessBoardAdapter {
    #[must_use]
    pub fn from_environment() -> Self {
        let command = env::var_os("GAREJI_BOARD_BIN")
            .map_or_else(|| PathBuf::from("gareji-board"), PathBuf::from);
        let database = env::var_os("GAREJI_BOARD_DB").map(PathBuf::from);
        Self::new(command, database)
    }

    #[must_use]
    pub fn new(command: PathBuf, database: Option<PathBuf>) -> Self {
        Self {
            command,
            database,
            process: Mutex::new(None),
            request_counter: AtomicU64::new(1),
        }
    }

    fn spawn(&self) -> Result<BoardProcess, BoardPortError> {
        let mut command = Command::new(&self.command);
        if let Some(database) = &self.database {
            command.arg("--database").arg(database);
        }
        let mut child = command
            .arg("bridge")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|_| BoardPortError::Unavailable)?;
        let stdin = child.stdin.take().ok_or(BoardPortError::Unavailable)?;
        let stdout = child.stdout.take().ok_or(BoardPortError::Unavailable)?;
        Ok(BoardProcess {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
        })
    }
}

impl BoardPort for ProcessBoardAdapter {
    fn assess_active_work(
        &self,
        project_id: &str,
        work_item_id: &str,
    ) -> Result<BoardActiveWorkAssessment, BoardPortError> {
        let counter = self.request_counter.fetch_add(1, Ordering::Relaxed);
        let request_id = format!("core-{}-{counter}", std::process::id());
        let request = BoardBridgeRequest {
            protocol_version: BOARD_BRIDGE_PROTOCOL_VERSION.to_owned(),
            request_id: request_id.clone(),
            operation: BoardBridgeOperation::AssessActiveWork {
                project_id: project_id.to_owned(),
                work_item_id: work_item_id.to_owned(),
            },
        };
        let mut process = self
            .process
            .lock()
            .map_err(|_| BoardPortError::Unavailable)?;
        if process.is_none() {
            *process = Some(self.spawn()?);
        }
        let response = process
            .as_mut()
            .ok_or(BoardPortError::Unavailable)?
            .exchange(&request);
        if response.is_err() {
            process.take();
        }
        let mapped = match response? {
            BoardBridgeResponse::Ok {
                protocol_version,
                request_id: response_id,
                result,
            } if protocol_version == BOARD_BRIDGE_PROTOCOL_VERSION && response_id == request_id => {
                decode_assessment(result, project_id, work_item_id)
            }
            BoardBridgeResponse::Error {
                protocol_version,
                request_id: response_id,
                error,
            } if protocol_version == BOARD_BRIDGE_PROTOCOL_VERSION && response_id == request_id => {
                Err(map_board_error(error.code))
            }
            _ => Err(BoardPortError::InvalidResponse),
        };
        if matches!(
            mapped,
            Err(BoardPortError::Unavailable | BoardPortError::InvalidResponse)
        ) {
            process.take();
        }
        mapped
    }
}

struct BoardProcess {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl BoardProcess {
    fn exchange(
        &mut self,
        request: &BoardBridgeRequest,
    ) -> Result<BoardBridgeResponse, BoardPortError> {
        let payload = serde_json::to_vec(request).map_err(|_| BoardPortError::InvalidResponse)?;
        if payload.len() > MAX_BRIDGE_MESSAGE_BYTES {
            return Err(BoardPortError::InvalidResponse);
        }
        self.stdin
            .write_all(&payload)
            .and_then(|()| self.stdin.write_all(b"\n"))
            .and_then(|()| self.stdin.flush())
            .map_err(|_| BoardPortError::Unavailable)?;
        let mut response = Vec::new();
        if !read_bounded_line(&mut self.stdout, &mut response)? {
            return Err(BoardPortError::Unavailable);
        }
        serde_json::from_slice(&response).map_err(|_| BoardPortError::InvalidResponse)
    }
}

impl Drop for BoardProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_bounded_line(
    reader: &mut impl BufRead,
    output: &mut Vec<u8>,
) -> Result<bool, BoardPortError> {
    output.clear();
    loop {
        let available = reader.fill_buf().map_err(|_| BoardPortError::Unavailable)?;
        if available.is_empty() {
            return Ok(!output.is_empty());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |position| position + 1);
        let payload_len = newline.unwrap_or(consumed);
        if output.len().saturating_add(payload_len) > MAX_BRIDGE_MESSAGE_BYTES {
            return Err(BoardPortError::InvalidResponse);
        }
        output.extend_from_slice(&available[..payload_len]);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(true);
        }
    }
}

#[derive(Debug, Serialize)]
struct BoardBridgeRequest {
    protocol_version: String,
    request_id: String,
    #[serde(flatten)]
    operation: BoardBridgeOperation,
}

#[derive(Debug, Serialize)]
#[serde(tag = "operation", content = "payload", rename_all = "snake_case")]
enum BoardBridgeOperation {
    AssessActiveWork {
        project_id: String,
        work_item_id: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum BoardBridgeResponse {
    Ok {
        protocol_version: String,
        request_id: String,
        result: Value,
    },
    Error {
        protocol_version: String,
        request_id: String,
        error: BoardBridgeError,
    },
}

#[derive(Debug, Deserialize)]
struct BoardBridgeError {
    code: BoardBridgeErrorCode,
    #[allow(dead_code)]
    message: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BoardBridgeErrorCode {
    InvalidRequest,
    ProjectNotFound,
    WorkItemNotFound,
    InternalError,
}

#[derive(Debug, Deserialize)]
struct BoardAssessment {
    project_id: String,
    work_item_id: String,
    #[allow(dead_code)]
    state: BoardWorkItemState,
    eligibility: BoardEligibility,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum BoardEligibility {
    Eligible,
    Ineligible {
        #[allow(dead_code)]
        reason: BoardIneligibleReason,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BoardIneligibleReason {
    NotAdmitted,
    Blocked,
    Terminal,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BoardWorkItemState {
    Backlog,
    Todo,
    InProgress,
    InReview,
    Blocked,
    Done,
    Cancelled,
}

fn decode_assessment(
    result: Value,
    expected_project_id: &str,
    expected_work_item_id: &str,
) -> Result<BoardActiveWorkAssessment, BoardPortError> {
    let assessment: BoardAssessment =
        serde_json::from_value(result).map_err(|_| BoardPortError::InvalidResponse)?;
    if assessment.project_id != expected_project_id
        || assessment.work_item_id != expected_work_item_id
    {
        return Err(BoardPortError::InvalidResponse);
    }
    Ok(match assessment.eligibility {
        BoardEligibility::Eligible => BoardActiveWorkAssessment::Eligible,
        BoardEligibility::Ineligible { .. } => BoardActiveWorkAssessment::Ineligible,
    })
}

const fn map_board_error(code: BoardBridgeErrorCode) -> BoardPortError {
    match code {
        BoardBridgeErrorCode::InvalidRequest => BoardPortError::InvalidResponse,
        BoardBridgeErrorCode::ProjectNotFound => BoardPortError::ProjectNotFound,
        BoardBridgeErrorCode::WorkItemNotFound => BoardPortError::WorkItemNotFound,
        BoardBridgeErrorCode::InternalError => BoardPortError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use serde_json::json;

    use super::*;

    #[test]
    fn missing_board_binary_fails_closed() {
        let adapter = ProcessBoardAdapter::new(
            PathBuf::from("gareji-board-binary-that-does-not-exist"),
            None,
        );
        assert_eq!(
            adapter.assess_active_work("core", "CORE-1"),
            Err(BoardPortError::Unavailable)
        );
    }

    #[test]
    fn board_response_reader_rejects_oversized_messages() {
        let mut input = Cursor::new(vec![b'x'; MAX_BRIDGE_MESSAGE_BYTES + 1]);
        let mut output = Vec::new();
        assert_eq!(
            read_bounded_line(&mut input, &mut output),
            Err(BoardPortError::InvalidResponse)
        );
    }

    #[test]
    fn assessment_must_repeat_the_requested_identities() {
        let result = json!({
            "project_id": "other",
            "work_item_id": "CORE-1",
            "state": "todo",
            "eligibility": { "status": "eligible" }
        });
        assert_eq!(
            decode_assessment(result, "core", "CORE-1"),
            Err(BoardPortError::InvalidResponse)
        );
    }
}
