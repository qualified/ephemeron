// Start the controller
use kube::Client;
use tracing_subscriber::fmt::format::FmtSpan;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> Result<()> {
    let domain = std::env::var("EPHEMERON_DOMAIN").expect("EPHEMERON_DOMAIN must be set");
    if domain.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "EPHEMERON_DOMAIN must not be empty",
        )
        .into());
    }

    let filter =
        std::env::var("RUST_LOG").unwrap_or_else(|_| "tracing=info,ephemeron=trace".to_owned());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .init();

    let client = Client::try_default().await?;
    ephemeron::run(client, domain).await;
    Ok(())
}
