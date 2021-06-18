// Provides Web API
use kube::Client;
use tracing_subscriber::fmt::format::FmtSpan;
use warp::{
    http::{header, Method},
    Filter,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> Result<()> {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info,ephemeron=debug".to_owned());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .init();

    let client = Client::try_default().await?;
    let cors = warp::cors()
        .allow_any_origin()
        .allow_headers(&[header::AUTHORIZATION, header::CONTENT_TYPE])
        .allow_methods(&[Method::POST, Method::GET, Method::DELETE, Method::OPTIONS]);
    let api = ephemeron::api::new(client).with(cors);
    warp::serve(api).run(([0, 0, 0, 0], 3030)).await;
    Ok(())
}
