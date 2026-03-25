use clap::Args;

#[derive(Args)]
pub struct ListPeersArgs {
    /// Session token for authentication
    #[arg(long)]
    pub token: Option<String>,
}

pub fn execute(_args: ListPeersArgs) -> i32 {
    // Stub — will be implemented in Step 6
    println!("list-peers: not yet implemented");
    0
}
