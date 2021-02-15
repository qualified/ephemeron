// Provides Web API
use kube::Client;
use tracing_subscriber::fmt::format::FmtSpan;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> Result<()> {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "ephemeron=trace".to_owned());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .init();

    let client = Client::try_default().await?;
    let api = ephemeron::api::new(client);
    warp::serve(api).run(([127, 0, 0, 1], 3030)).await;
    Ok(())
}
