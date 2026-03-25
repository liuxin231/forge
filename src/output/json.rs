use crate::supervisor::protocol::ServiceStatus;
use anyhow::Result;

pub fn print_services(statuses: &[ServiceStatus]) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(statuses)?);
    Ok(())
}
