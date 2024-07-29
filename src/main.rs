use crate::config::*;
use crate::multiplier::Multiplier;
use crate::multiplier::MultiplierNote;
use crate::multiplier::Multipliers;
use crate::round::run_rounds;
use crate::round::RoundManager;
use crate::routes::*;
use crate::subscriber::start_invoice_subscription;
use crate::zapper::start_zapper;
use crate::zapper::LndZapper;
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
use nostr_sdk::Client;
use nostr_sdk::Options;
use serde::Deserialize;
use serde::Serialize;
use serde_json::from_reader;
use serde_json::to_string;
use sled::Db;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use tokio::spawn;
use tonic_openssl_lnd::lnrpc::GetInfoRequest;
use tonic_openssl_lnd::lnrpc::GetInfoResponse;
use tonic_openssl_lnd::LndLightningClient;
use tonic_openssl_lnd::LndRouterClient;
use tower_http::cors::Any;
use tower_http::cors::CorsLayer;
use tracing::level_filters::LevelFilter;
use yaml_rust2::YamlLoader;

mod config;
mod db;
mod logger;
mod multiplier;
mod round;
mod routes;
mod subscriber;
mod zapper;

pub const MAIN_KEY_NAME: &str = "roll";
pub const NONCE_KEY_NAME: &str = "nonce";

#[derive(Clone)]
pub struct State {
    pub db: Db,
    pub lightning_client: LndLightningClient,
    pub router_client: LndRouterClient,
    pub main_keys: Keys,
    pub nonce_keys: Keys,
    pub domain: String,
    pub route_hints: bool,
    pub client: Client,
    pub multipliers: Multipliers,
    pub relays: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config: Config = Config::parse();

    logger::init_tracing(LevelFilter::DEBUG, config.json)?;

    let relays = config.clone().relay;

    let mut lnd_client = tonic_openssl_lnd::connect(
        config.lnd_host.clone(),
        config.lnd_port,
        config.cert_file(),
        config.macaroon_file(),
    )
    .await
    .expect("failed to connect");

    let mut ln_client = lnd_client.lightning().clone();
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

    let (main_keys_path, nonce_keys_path) = {
        let mut main_keys_path = path.clone();
        main_keys_path.push("main-keys.json");

        let mut nonce_keys_path = path.clone();
        nonce_keys_path.push("nonce-keys.json");

        (main_keys_path, nonce_keys_path)
    };

    let main_keys = get_keys(main_keys_path);
    let nonce_keys = get_keys(nonce_keys_path);

    let options = Options::default();
    // Create new client
    let client = Client::with_opts(
        &main_keys,
        options
            .wait_for_send(true)
            .send_timeout(Some(Duration::from_secs(20))),
    );
    client.add_relays(relays.clone()).await?;

    let sender = start_zapper(lnd_client.router().clone());
    let lnd_zapper = LndZapper { sender };

    client.set_zapper(lnd_zapper).await;
    client.connect().await;

    let multipliers = {
        let path = PathBuf::from(&config.multipliers_file);
        let mut file = File::open(path).expect("Failed to open multiplier config file");
        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .expect("Failed to read multiplier config file");

        let docs =
            YamlLoader::load_from_str(&contents).expect("Failed to parse multiplier config file");

        let doc = &docs[0];

        // TODO: We should verify that the provided note IDs exist, parse the contents and ensure
        // that they represent their multiplier faithfully.

        Multipliers([
            MultiplierNote {
                multiplier: Multiplier::X1_05,
                note_id: doc["x1_05"].clone().into_string().expect("1_05"),
            },
            MultiplierNote {
                multiplier: Multiplier::X1_1,
                note_id: doc["x1_1"].clone().into_string().expect("1_1"),
            },
            MultiplierNote {
                multiplier: Multiplier::X1_33,
                note_id: doc["x1_33"].clone().into_string().expect("1_33"),
            },
            MultiplierNote {
                multiplier: Multiplier::X1_5,
                note_id: doc["x1_5"].clone().into_string().expect("1_5"),
            },
            MultiplierNote {
                multiplier: Multiplier::X2,
                note_id: doc["x2"].clone().into_string().expect("2"),
            },
            MultiplierNote {
                multiplier: Multiplier::X3,
                note_id: doc["x3"].clone().into_string().expect("3"),
            },
            MultiplierNote {
                multiplier: Multiplier::X10,
                note_id: doc["x10"].clone().into_string().expect("10"),
            },
            MultiplierNote {
                multiplier: Multiplier::X25,
                note_id: doc["x25"].clone().into_string().expect("25"),
            },
            MultiplierNote {
                multiplier: Multiplier::X50,
                note_id: doc["x50"].clone().into_string().expect("50"),
            },
            MultiplierNote {
                multiplier: Multiplier::X100,
                note_id: doc["x100"].clone().into_string().expect("100"),
            },
            MultiplierNote {
                multiplier: Multiplier::X1000,
                note_id: doc["x1000"].clone().into_string().expect("1000"),
            },
        ])
    };

    let state = State {
        db,
        lightning_client: lnd_client.lightning().clone(),
        router_client: lnd_client.router().clone(),
        main_keys: main_keys.clone(),
        nonce_keys: nonce_keys.clone(),
        domain: config.domain.clone(),
        route_hints: config.route_hints,
        client: client.clone(),
        multipliers: multipliers.clone(),
        relays,
    };

    let addr: std::net::SocketAddr = format!("{}:{}", config.bind, config.port)
        .parse()
        .expect("Failed to parse bind/port for webserver");

    tracing::info!("Webserver running on http://{}", addr);

    let server_router = Router::new()
        .route("/get-invoice/:hash", get(get_invoice))
        .route("/.well-known/lnurlp/:name", get(get_lnurl_pay))
        .route("/.well-known/nostr.json", get(get_nip05))
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
        main_keys.clone(),
        client.clone(),
    ));

    // TODO: add a way to stop a round, so we do not accidentally stop a round with active bets on
    // them.
    spawn({
        let client = client.clone();
        async move {
            let round_manager = RoundManager::new(client.clone(), nonce_keys.clone(), multipliers);
            if let Err(e) = run_rounds(
                state.db.clone(),
                round_manager,
                Duration::from_secs(config.round_interval_seconds as u64),
            )
            .await
            {
                tracing::error!("Stopped rolling dice: {e:#}");
            }
        }
    });

    let graceful = server.with_graceful_shutdown(async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to create Ctrl+C shutdown signal");
    });

    // Await the server to receive the shutdown signal
    if let Err(e) = graceful.await {
        tracing::error!("shutdown error: {}", e);
    }

    client.disconnect().await?;

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
