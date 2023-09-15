use std::env;
use std::str::FromStr;

use anyhow::Context;
use axum::extract;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Extension, Router};
use rss::{ChannelBuilder, Image, ItemBuilder};
use serde::Deserialize;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool},
    Sqlite,
};
use sqlx::{ConnectOptions, Pool};
use tower_http::services::ServeDir;
use tower_http::trace::{self, TraceLayer};
use tracing::Level;

const DEFAULT_LISTEN_PORT: &str = "3000";
const DEFAULT_DOMAIN: &str = "localhost";
const LISTEN_IFACE: &str = "0.0.0.0";

const ASSETS_PATH: &str = "assets";
const FEED: &str = "/feed.xml";
const IMAGE: &str = "link-solid.png";

fn default_pub_date() -> chrono::NaiveDateTime {
    chrono::Utc::now().naive_utc()
}

#[derive(Deserialize)]
struct Item {
    link: String,

    // NOTE: We can't make it non-naive when using `query_as!`:
    // https://github.com/launchbadge/sqlx/issues/2288
    #[serde(default = "default_pub_date")]
    pub_date: chrono::NaiveDateTime,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .compact()
        .init();

    // https://github.com/Bodobolero/axum_crud_api/blob/master/src/main.rs
    let pool = prepare_database().await?;

    let app = Router::new()
        .route(FEED, get(feed))
        .route("/add", post(add_item))
        .nest_service(&(format!("/{}", ASSETS_PATH)), ServeDir::new(ASSETS_PATH))
        .layer(Extension(pool))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
                .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
        );

    let listen_port = env::var("LISTEN_PORT").unwrap_or_else(|_| {
        tracing::warn!(
            "Could not get `LISTEN_PORT`, defaulting to {}.",
            DEFAULT_LISTEN_PORT
        );
        DEFAULT_LISTEN_PORT.to_string()
    });

    let addr = format!("{}:{}", LISTEN_IFACE, listen_port);

    tracing::info!("Listening on {}", addr);

    axum::Server::bind(&addr.parse().unwrap())
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

async fn prepare_database() -> anyhow::Result<Pool<Sqlite>> {
    let db_url = env::var("DATABASE_URL")?;
    let conn = SqliteConnectOptions::from_str(&db_url)?
        .journal_mode(SqliteJournalMode::Wal)
        .create_if_missing(true)
        .connect()
        .await?;
    sqlx::Connection::close(conn);

    // prepare connection pool
    let pool = SqlitePoolOptions::new()
        .max_connections(50)
        .connect(&db_url)
        .await
        .context("could not connect to DATABASE_URL")?;

    // prepare schema in db if it does not yet exist
    sqlx::migrate!().run(&pool).await?;

    Ok(pool)
}

async fn feed(Extension(pool): Extension<SqlitePool>) -> impl IntoResponse {
    // If you need it, here's the RSS 2.0 specification:
    // https://www.rssboard.org/rss-draft-1
    let domain = env::var("DOMAIN").unwrap_or_else(|_| {
        tracing::warn!(
            "Could not get `DOMAIN` environment variable, defaulting to `{}`.",
            DEFAULT_DOMAIN
        );
        DEFAULT_DOMAIN.to_string()
    });

    let mut image = Image::default();
    image.set_url(format!("{}/{}/{}", domain, ASSETS_PATH, IMAGE));

    // NOTE: We could stream, but it's not worth for 50 items.
    let result = sqlx::query_as!(
        Item,
        r#"
            SELECT link, pub_date
            FROM items
            LIMIT 50
        "#
    )
    .fetch_all(&pool)
    .await;

    match result {
        Ok(result) => {
            // TODO: Can we deserialize directly to rss::Item?
            let items: Vec<rss::Item> = result
                .into_iter()
                .map(|row| {
                    ItemBuilder::default()
                        .link(row.link)
                        .pub_date(row.pub_date.and_utc().to_rfc2822())
                        .build()
                })
                .collect();

            let channel = ChannelBuilder::default()
                .title("Aldur's zap-it-later ⚡")
                .link(domain)
                .description("Web link to an RSS feed.")
                .image(Some(image))
                .items(items)
                .build();

            channel.write_to(::std::io::sink()).unwrap(); // write to the channel to a writer
            (StatusCode::OK, channel.to_string())
        }
        Err(err) => {
            // TODO: Return XML?
            tracing::error!("error retrieving items: {:?}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error retrieving tasks from DB.".to_string(),
            )
        }
    }
}

async fn add_item(
    Extension(pool): Extension<SqlitePool>,
    extract::Json(payload): extract::Json<Item>,
) -> impl IntoResponse {
    let result = sqlx::query_scalar!(
        "INSERT INTO items (link, pub_date) VALUES (?, ?) RETURNING id",
        payload.link,
        payload.pub_date,
    )
    .fetch_one(&pool)
    .await;

    match result {
        Ok(id) => (StatusCode::OK, format!("OK - {id}")),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
