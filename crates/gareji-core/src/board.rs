//! Board-owned active-work assessment seam.

/// The only Board decision Core needs before storing an active Work item reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoardActiveWorkAssessment {
    Eligible,
    Ineligible,
}

/// Port implemented by the production Board process Adapter and in-memory tests.
pub trait BoardPort {
    /// Ask Board whether one Work item belongs to the project and may be active.
    fn assess_active_work(
        &self,
        project_id: &str,
        work_item_id: &str,
    ) -> Result<BoardActiveWorkAssessment, BoardPortError>;
}

/// Stable failure classes at the Core-to-Board seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoardPortError {
    ProjectNotFound,
    WorkItemNotFound,
    Unavailable,
    InvalidResponse,
}

/// Fail-closed Adapter used when no Board process is configured.
#[derive(Default)]
pub struct UnavailableBoard;

impl BoardPort for UnavailableBoard {
    fn assess_active_work(
        &self,
        _project_id: &str,
        _work_item_id: &str,
    ) -> Result<BoardActiveWorkAssessment, BoardPortError> {
        Err(BoardPortError::Unavailable)
    }
}
