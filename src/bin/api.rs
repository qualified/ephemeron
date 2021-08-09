// Provides Web API
use ephemeron::api::Config;
use kube::Client;
use snafu::{ResultExt, Snafu};
use tracing_subscriber::fmt::format::FmtSpan;
use warp::{
    http::{header, Method},
    Filter,
};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to read config: {}", source))]
    ReadConfig { source: std::io::Error },

    #[snafu(display("Failed to parse config: {}", source))]
    ParseConfig { source: serde_yaml::Error },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_owned());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .init();

    let client = Client::try_default().await?;
    let config = get_config()?;
    let cors = warp::cors()
        .allow_any_origin()
        .allow_headers(&[header::AUTHORIZATION, header::CONTENT_TYPE])
        .allow_methods(&[
            Method::DELETE,
            Method::GET,
            Method::OPTIONS,
            Method::PATCH,
            Method::POST,
        ]);
    let api = ephemeron::api::new(client, config).with(cors);
    warp::serve(api).run(([0, 0, 0, 0], 3030)).await;
    Ok(())
}

fn get_config() -> Result<Config, Error> {
    let config_path =
        std::env::var("EPHEMERON_CONFIG").unwrap_or_else(|_| "config.yaml".to_owned());
    let config = std::fs::read(config_path).context(ReadConfig)?;
    serde_yaml::from_slice(&config).context(ParseConfig)
}
