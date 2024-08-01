use crate::config::*;
use crate::multiplier::Multiplier;
use crate::multiplier::MultiplierNote;
use crate::multiplier::Multipliers;
use crate::nonce::manage_nonces;
use crate::routes::*;
use crate::social_updates::post_social_updates;
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
use tokio::sync::broadcast;
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
mod nonce;
mod payouts;
mod routes;
mod social_updates;
mod subscriber;
mod utils;
mod zapper;

pub const MAIN_KEY_NAME: &str = "main";
pub const NONCE_KEY_NAME: &str = "nonce";
pub const SOCIAL_KEY_NAME: &str = "social";

#[derive(Clone)]
pub struct State {
    pub db: Db,
    pub lightning_client: LndLightningClient,
    pub router_client: LndRouterClient,
    /// The keys for the account posting the multiplier notes
    pub main_keys: Keys,
    /// The keys for the account posting the nonce notes
    pub nonce_keys: Keys,
    /// The keys for a social media account posting game unrelated posts
    pub social_keys: Keys,
    pub domain: String,
    pub route_hints: bool,
    pub client: Client,
    pub multipliers: Multipliers,
    pub relays: Vec<String>,
    pub reveal_nonce_after_secs: u64,
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

    let (main_keys_path, nonce_keys_path, social_keys_path) = {
        let mut main_keys_path = path.clone();
        main_keys_path.push("main-keys.json");

        let mut nonce_keys_path = path.clone();
        nonce_keys_path.push("nonce-keys.json");

        let mut social_keys_path = path.clone();
        social_keys_path.push("social-keys.json");

        (main_keys_path, nonce_keys_path, social_keys_path)
    };

    let main_keys = get_keys(main_keys_path);
    let nonce_keys = get_keys(nonce_keys_path);
    let social_keys = get_keys(social_keys_path);

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
        social_keys: social_keys.clone(),
        domain: config.domain.clone(),
        route_hints: config.route_hints,
        client: client.clone(),
        multipliers: multipliers.clone(),
        relays,
        reveal_nonce_after_secs: config.reveal_nonce_after_secs as u64,
    };

    let addr: std::net::SocketAddr = format!("{}:{}", config.bind, config.port)
        .parse()
        .expect("Failed to parse bind/port for webserver");

    tracing::info!("Webserver running on http://{}", addr);

    let server_router = Router::new()
        .route("/get-invoice-for-game/:hash", get(get_invoice_for_game))
        .route("/get-invoice-for-zap/:hash", get(get_invoice_for_zap))
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

    let (ctrl_c_tx, mut ctrl_c_rx) = {
        let (tx, rx) = broadcast::channel(1);
        let tx_clone = tx.clone();
        spawn(async move {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to listen for Ctrl+C shutdown signal");
            tracing::warn!("Ctrl-C pressed; sending stop");
            tx_clone.send(()).expect("failed to send Ctrl+C signal via broadcast channel");
        });
        (tx, rx)
    };

    let manage_nonces = spawn(manage_nonces(
        client.clone(),
        nonce_keys.clone(),
        state.db.clone(),
        config.expire_nonce_after_secs as u64,
        config.reveal_nonce_after_secs as u64,
        ctrl_c_tx.subscribe()
    ));

    // Invoice event stream
    spawn(start_invoice_subscription(
        state.db.clone(),
        state.lightning_client.clone(),
        main_keys.clone(),
        client.clone(),
        multipliers.clone(),
    ));

    // Post social updates about winners
    spawn(post_social_updates(
        client.clone(),
        social_keys.clone(),
        state.db.clone(),
        multipliers,
        main_keys.public_key(),
        nonce_keys.public_key(),
        config.social_updates_time_window_minutes,
    ));

    let graceful = server.with_graceful_shutdown(async {
        let _ = ctrl_c_rx.recv().await;
    });

    // Await the server to receive the shutdown signal

    let (graceful, manage_nonces) = tokio::join!(graceful, manage_nonces);

    if let Err(e) = graceful {
        tracing::error!("shutdown error in server: {}", e);
    }

    match manage_nonces {
        Ok(Err(e)) => tracing::error!("shutdown error in manage_nonces task: {}", e),
        Err(e) => tracing::error!("shutdown error in manage_nonces task: {}", e),
        _ => (),
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
