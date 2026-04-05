use crate::config::teams;
use crate::phone::manager;
use crate::phone::types::{AgentInfo, PhoneMessage};

#[tauri::command]
pub async fn phone_send_message(
    from: String,
    to: String,
    body: String,
    team: String,
) -> Result<String, String> {
    let discovered = teams::discover_teams();
    manager::send_message(&from, &to, &body, &team, &discovered)
}

#[tauri::command]
pub async fn phone_get_inbox(agent_name: String) -> Result<Vec<PhoneMessage>, String> {
    manager::get_inbox(&agent_name)
}

#[tauri::command]
pub async fn phone_list_agents() -> Result<Vec<AgentInfo>, String> {
    let discovered = teams::discover_teams();
    Ok(manager::list_agents(&discovered))
}

#[tauri::command]
pub async fn phone_ack_messages(
    agent_name: String,
    message_ids: Vec<String>,
) -> Result<(), String> {
    manager::ack_messages(&agent_name, &message_ids)
}
