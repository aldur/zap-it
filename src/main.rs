use std::env;
use std::str::FromStr;

use anyhow::Context;
use axum::extract::{self, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use rss::{ChannelBuilder, Image, ItemBuilder};
use serde::Deserialize;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::Pool;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool},
    Sqlite,
};
use tower_http::services::ServeDir;
use tower_http::trace::{self, TraceLayer};
use tracing::Level;
use validator::Validate;

const DEFAULT_DATABASE_URL: &str = "sqlite:db.sqlite";
const DEFAULT_LISTEN_PORT: &str = "3000";
const DEFAULT_DOMAIN: &str = "localhost";
const DEFAULT_LISTEN_IFACE: &str = "0.0.0.0";

const ASSETS_PATH: &str = "assets";
const FEED: &str = "/feed.xml";
const IMAGE: &str = "link-solid.png";

struct Config {
    database_url: String,
    listen_addr: String,
    domain: String,
}

impl Config {
    fn from_env() -> Self {
        let database_url = env::var("DATABASE_URL").unwrap_or_else(|_| {
            let default_url = DEFAULT_DATABASE_URL.to_string();
            tracing::warn!(
                "`DATABASE_URL` not set, defaulting to `{}`",
                DEFAULT_DATABASE_URL
            );
            default_url
        });

        let listen_port = env::var("LISTEN_PORT").unwrap_or_else(|_| {
            let default_listen_port = DEFAULT_LISTEN_PORT.to_string();
            tracing::warn!(
                "Listen port not set, defaulting to `{}`",
                default_listen_port
            );
            default_listen_port
        });

        let listen_iface = env::var("LISTEN_IFACE").unwrap_or_else(|_| {
            let default_list_iface = DEFAULT_LISTEN_IFACE.to_string();
            tracing::warn!(
                "Listen interface not set, default to `{}`",
                default_list_iface
            );
            default_list_iface
        });

        let domain = env::var("DOMAIN").unwrap_or_else(|_| {
            let default_domain = DEFAULT_DOMAIN.to_string();
            tracing::warn!("`DOMAIN` not set, defaulting to `{}`", default_domain);
            default_domain
        });

        Self {
            database_url,
            listen_addr: format!("{listen_iface}:{listen_port}"),
            domain,
        }
    }
}

fn default_pub_date() -> chrono::NaiveDateTime {
    chrono::Utc::now().naive_utc()
}

#[derive(Deserialize, Validate)]
struct Item {
    title: String,

    #[validate(url)]
    link: String,

    // NOTE: We can't make it non-naive when using `query_as!`:
    // https://github.com/launchbadge/sqlx/issues/2288
    #[serde(default = "default_pub_date")]
    pub_date: chrono::NaiveDateTime,
}

#[derive(Clone)] // https://github.com/tokio-rs/axum/discussions/2254
struct AppState {
    pool: SqlitePool,
    domain: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .compact()
        .init();

    let config = Config::from_env();

    // https://github.com/Bodobolero/axum_crud_api/blob/master/src/main.rs
    let pool = prepare_database(&config.database_url).await?;

    let shared_state = AppState {
        pool,
        domain: config.domain,
    };

    let app = Router::new()
        .route(FEED, get(feed))
        .route("/add", post(add_item))
        .with_state(shared_state)
        .nest_service(&(format!("/{ASSETS_PATH}")), ServeDir::new(ASSETS_PATH))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
                .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
        );

    tracing::info!("Listening on {}...", config.listen_addr);
    let listener = tokio::net::TcpListener::bind(config.listen_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn prepare_database(db_url: &str) -> anyhow::Result<Pool<Sqlite>> {
    let options = SqliteConnectOptions::from_str(db_url)?
        .journal_mode(SqliteJournalMode::Wal)
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(50)
        .connect_with(options)
        .await
        .with_context(|| format!("could not connect to DATABASE_URL '{db_url}'"))?;

    sqlx::migrate!().run(&pool).await?;

    Ok(pool)
}

enum FeedError {
    Database(sqlx::Error),
}

// Implement IntoResponse to convert the error into a response
impl IntoResponse for FeedError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            FeedError::Database(e) => {
                tracing::error!("Database error generating feed: {:?}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Error generating feed".to_string(),
                )
            }
        };
        (status, message).into_response()
    }
}

impl From<sqlx::Error> for FeedError {
    fn from(e: sqlx::Error) -> Self {
        FeedError::Database(e)
    }
}

async fn feed(State(state): State<AppState>) -> Result<impl IntoResponse, FeedError> {
    // If you need it, here's the RSS 2.0 specification:
    // https://www.rssboard.org/rss-draft-1
    let mut image = Image::default();
    image.set_link(&(state.domain));
    image.set_title("Link icon");
    image.set_url(format!("{}/{}/{}", &(state.domain), ASSETS_PATH, IMAGE));

    // NOTE: We could stream, but it's not worth for 50 items.
    let result = sqlx::query_as!(
        Item,
        r#"
            SELECT title, link, pub_date
            FROM items
            ORDER BY pub_date DESC
            LIMIT 50
        "#
    )
    .fetch_all(&state.pool)
    .await?;

    let items: Vec<rss::Item> = result
        .into_iter()
        .map(|row| {
            ItemBuilder::default()
                .title(row.title)
                .link(row.link)
                .pub_date(row.pub_date.and_utc().to_rfc2822())
                .build()
        })
        .collect();

    let channel = ChannelBuilder::default()
        .title("Aldur's ZapIt ⚡")
        .link(&(state.domain))
        .description("Web link to an RSS feed.")
        .image(Some(image))
        .items(items)
        .build();

    Ok((
        StatusCode::OK,
        [("Content-Type", "application/rss+xml; charset=utf-8")],
        channel.to_string(),
    ))
}

enum AddItemError {
    Conflict(String),
    Internal(String),
}

impl IntoResponse for AddItemError {
    fn into_response(self) -> Response {
        match self {
            AddItemError::Conflict(body) => (StatusCode::CONFLICT, body).into_response(),
            AddItemError::Internal(body) => {
                (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
            }
        }
    }
}

async fn add_item(
    State(state): State<AppState>,
    extract::Json(payload): extract::Json<Item>,
) -> Result<impl IntoResponse, AddItemError> {
    payload
        .validate()
        .map_err(|e| AddItemError::Internal(e.to_string()))?;

    let result = sqlx::query_scalar!(
        "INSERT INTO items (title, link, pub_date) VALUES (?, ?, ?) RETURNING id",
        payload.title,
        payload.link,
        payload.pub_date,
    )
    .fetch_one(&state.pool)
    .await;

    let id = result.map_err(|e| match e {
        sqlx::Error::Database(dbe) if dbe.is_unique_violation() => {
            AddItemError::Conflict("🦦 Already zapped!".to_owned())
        }
        _ => AddItemError::Internal(e.to_string()),
    })?;

    Ok((StatusCode::CREATED, format!("⚡zap #{id}")))
}
