//! GraphQL mutations (review submission). Filled in with the outbox.

use serde_json::Value;

use crate::gql::{GqlError, GqlResult};
use crate::model::World;

pub fn handle(_w: &mut World, _viewer: &str, op: &str, _vars: &Value) -> GqlResult {
    Err(GqlError::NotFound(format!("fake-github doesn't implement operation {op}")))
}
