use crate::config::*;
use crate::dice::start_rounds;
use crate::dice::LndZapper;
use crate::routes::*;
use crate::subscriber::start_invoice_subscription;
use crate::zapper::start_zapper;
use axum::http;
use axum::http::Method;
use axum::http::StatusCode;
use axum::http::Uri;
use axum::routing::get;
use axum::Extension;
use axum::Router;
use clap::Parser;
use nostr::prelude::ToBech32;
use nostr::Keys;
use serde::Deserialize;
use serde::Serialize;
use serde_json::from_reader;
use serde_json::to_string;
use sled::Db;
use std::fs::File;
use std::io::BufReader;
use std::io::Write;
use std::path::PathBuf;
use tokio::spawn;
use tonic_openssl_lnd::lnrpc::GetInfoRequest;
use tonic_openssl_lnd::lnrpc::GetInfoResponse;
use tonic_openssl_lnd::LndLightningClient;
use tonic_openssl_lnd::LndRouterClient;
use tower_http::cors::Any;
use tower_http::cors::CorsLayer;
use tracing::level_filters::LevelFilter;

mod config;
mod db;
mod dice;
mod logger;
mod routes;
mod subscriber;
mod zapper;

#[derive(Clone)]
pub struct State {
    pub db: Db,
    pub lightning_client: LndLightningClient,
    pub router_client: LndRouterClient,
    pub keys: Keys,
    pub domain: String,
    pub route_hints: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logger::init_tracing(LevelFilter::DEBUG, false)?;

    let config: Config = Config::parse();

    let relays = config.clone().relay;

    let mut client = tonic_openssl_lnd::connect(
        config.lnd_host.clone(),
        config.lnd_port,
        config.cert_file(),
        config.macaroon_file(),
    )
    .await
    .expect("failed to connect");

    let mut ln_client = client.lightning().clone();
    let lnd_info: GetInfoResponse = ln_client
        .get_info(GetInfoRequest {})
        .await
        .expect("Failed to get lnd info")
        .into_inner();

    tracing::info!("Connected to LND: {}", lnd_info.identity_pubkey);

    // Create the datadir if it doesn't exist
    let path = PathBuf::from(&config.data_dir);
    std::fs::create_dir_all(path.clone())?;

    let db_path = {
        let mut path = path.clone();
        path.push("zaps.db");
        path
    };

    // DB management
    let db = sled::open(&db_path)?;

    let keys_path = {
        let mut path = path.clone();
        path.push("keys.json");
        path
    };

    let keys = get_keys(keys_path);

    let state = State {
        db,
        lightning_client: client.lightning().clone(),
        router_client: client.router().clone(),
        keys: keys.clone(),
        domain: config.domain.clone(),
        route_hints: config.route_hints,
    };

    let addr: std::net::SocketAddr = format!("{}:{}", config.bind, config.port)
        .parse()
        .expect("Failed to parse bind/port for webserver");

    tracing::info!("Webserver running on http://{}", addr);

    let server_router = Router::new()
        .route("/get-invoice/:hash", get(get_invoice))
        .route("/.well-known/lnurlp/:name", get(get_lnurl_pay))
        .fallback(fallback)
        .layer(Extension(state.clone()))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_headers(vec![http::header::CONTENT_TYPE])
                .allow_methods([Method::GET, Method::POST]),
        );

    let server = axum::Server::bind(&addr).serve(server_router.into_make_service());

    // Invoice event stream
    spawn(start_invoice_subscription(
        state.db.clone(),
        state.lightning_client.clone(),
        keys.clone(),
        relays.clone(),
    ));

    let sender = start_zapper(client.router().clone());
    let lnd_zapper = LndZapper { sender };

    spawn(start_rounds(state.db.clone(), keys, relays, lnd_zapper));

    let graceful = server.with_graceful_shutdown(async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to create Ctrl+C shutdown signal");
    });

    // Await the server to receive the shutdown signal
    if let Err(e) = graceful.await {
        tracing::error!("shutdown error: {}", e);
    }

    Ok(())
}

async fn fallback(uri: Uri) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, format!("No route for {}", uri))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct NostrKeys {
    server_key: String,
}

impl NostrKeys {
    fn generate() -> Self {
        let server_key = Keys::generate();

        NostrKeys {
            server_key: server_key.secret_key().unwrap().to_bech32().unwrap(),
        }
    }
}

fn get_keys(path: PathBuf) -> Keys {
    match File::open(&path) {
        Ok(file) => {
            let reader = BufReader::new(file);
            let n: NostrKeys = from_reader(reader).expect("Could not parse JSON");

            Keys::parse(n.server_key).expect("Could not parse key")
        }
        Err(_) => {
            let keys = NostrKeys::generate();
            let json_str = to_string(&keys).expect("Could not serialize data");

            let mut file = File::create(path).expect("Could not create file");
            file.write_all(json_str.as_bytes())
                .expect("Could not write to file");

            Keys::parse(&keys.server_key).expect("Could not parse key")
        }
    }
}
