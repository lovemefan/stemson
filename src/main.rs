//! Application entry point, the Rust counterpart of `backend/app.py`.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    sanicboot::run().await
}
