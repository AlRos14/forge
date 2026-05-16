#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assignee {
    Agent(String),
    User(String),
}
