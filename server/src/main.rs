//! The service: it holdeth the accounts, fetcheth the timetables, and
//! publisheth a calendar for each.

mod admin;
mod auth;
mod config;
mod crypto;
mod db;
mod feed;
mod google;
mod guard;
mod mfa;
mod people;
mod post;
mod schools;
mod sync;
mod web;
mod words;

use anyhow::{Context, Result};
use axum::Router;
use axum::routing::{get, post};
use clap::{Parser, Subcommand};
use sqlx::PgPool;
use std::sync::Arc;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use config::Config;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    /// Counts misses at the feed route, and turns away whoever is guessing.
    pub guard: Arc<guard::Guard>,
    /// How to reach people who are not looking at the page. None, and nobody
    /// is told anything, which is the default.
    pub mail: Option<Arc<post::Mailer>>,
    pub config: Arc<Config>,
    /// One client, so connections to Supabase are kept and reused.
    pub http: reqwest::Client,
}

#[derive(Parser)]
#[command(name = "stundenglas-server", version, about = "Multi-user WebUntis calendar service")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Print a fresh encryption key for STUNDENGLAS_KEY.
    GenerateKey,
    /// Attach a WebUntis login to a user and print its calendar URL.
    AddAccount {
        #[arg(long)]
        user: Uuid,
        #[arg(long)]
        server: String,
        #[arg(long)]
        school: String,
        #[arg(long)]
        username: String,
        /// The zone the school keeps its clocks in.
        #[arg(long, default_value = "Europe/Vienna")]
        timezone: chrono_tz::Tz,
        /// Read from the UNTIS_PASS environment variable, never the command line.
        #[arg(long, default_value = "UNTIS_PASS")]
        password_env: String,
    },
    /// Show a user's accounts and their calendar URLs.
    List {
        #[arg(long)]
        user: Uuid,
    },
    /// Attach a Google refresh token obtained elsewhere, and name the calendar.
    LinkGoogle {
        #[arg(long)]
        account: Uuid,
        #[arg(long, default_value = "Schule (Untis)")]
        calendar: String,
        /// Read from this environment variable, never the command line.
        #[arg(long, default_value = "GOOGLE_REFRESH_TOKEN")]
        token_env: String,
    },
    /// Refresh every account once and exit.
    SyncNow,
    /// Re-seal every stored secret under a new key.
    ///
    /// Set STUNDENGLAS_KEY to the new key, STUNDENGLAS_KEY_OLD to the one
    /// being retired, and STUNDENGLAS_KEY_VERSION one higher than it was.
    /// Safe to run again: rows already at the new version are passed over.
    RotateKey,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Wanted before a database exists, so it stands apart from everything else.
    if matches!(args.command, Some(Command::GenerateKey)) {
        println!("{}", crypto::Sealer::generate_key());
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stundenglas_server=info,tower_http=warn".into()),
        )
        .init();

    let config = Config::from_env()?;
    let pool = db::connect(&config.database_url).await?;
    let state = AppState {
        pool,
        guard: Arc::new(guard::Guard::default()),
        mail: post::Mailer::from_env()?.map(Arc::new),
        config: Arc::new(config),
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .context("building the HTTP client")?,
    };

    match args.command {
        Some(Command::GenerateKey) => unreachable!("handled above"),
        Some(Command::AddAccount { user, server, school, username, timezone, password_env }) => {
            let password = std::env::var(&password_env)
                .with_context(|| format!("{password_env} is not set"))?;
            let url = admin::add_account(
                &state,
                &db::NewAccount { user_id: user, server, school, username, password, timezone },
            )
            .await?;
            println!("{url}");
        }
        Some(Command::List { user }) => admin::list(&state, user).await?,
        Some(Command::LinkGoogle { account, calendar, token_env }) => {
            let refresh =
                std::env::var(&token_env).with_context(|| format!("{token_env} is not set"))?;
            google::adopt_refresh_token(&state, account, &refresh, &calendar).await?;
            println!("linked; the next refresh will push");
        }
        Some(Command::RotateKey) => {
            let Some(old) = state.config.retiring.clone() else {
                anyhow::bail!(
                    "STUNDENGLAS_KEY_OLD is not set, so there is no key to retire. \
                     Set it to the old key, STUNDENGLAS_KEY to the new one, and \
                     STUNDENGLAS_KEY_VERSION one higher than it was"
                );
            };
            if old.version() >= state.config.sealer.version() {
                anyhow::bail!(
                    "STUNDENGLAS_KEY_VERSION is still {}; raise it, or the rotation \
                     cannot tell a re-sealed row from one yet to do",
                    state.config.sealer.version()
                );
            }
            let (done, stuck) = db::rotate_key(&state.pool, &old, &state.config.sealer).await?;
            println!(
                "re-sealed {done} secret(s) under key version {}",
                state.config.sealer.version()
            );
            if stuck > 0 {
                println!(
                    "{stuck} could not be opened by the retiring key and were left alone; \
                     see the log for which"
                );
            }
        }
        Some(Command::SyncNow) => {
            let done = sync::once(state).await?;
            println!("refreshed {done} account(s)");
        }
        None => serve(state).await?,
    }
    Ok(())
}

async fn serve(state: AppState) -> Result<()> {
    tracing::info!(config = ?state.config, "starting");
    if state.mail.is_none() {
        tracing::info!("no SMTP_URL; nobody will be told anything by email");
    }
    let scheduler = tokio::spawn(sync::run(state.clone()));

    let listen = state.config.listen.clone();
    let app = Router::new()
        .route("/", get(web::index))
        .route("/signup", post(web::sign_up))
        .route("/login", post(web::sign_in))
        .route("/logout", post(web::sign_out))
        .route("/links", post(web::add_link))
        .route("/links/{id}/feeds", post(web::new_feed))
        .route("/links/{id}/delete", post(web::drop_link))
        .route("/links/{id}/password", post(web::new_password))
        .route("/feeds/{id}/settings", post(web::feed_settings))
        .route("/feeds/{id}/rotate", post(web::rotate_feed))
        .route("/feeds/{id}/delete", post(web::drop_feed))
        .route("/links/{id}/google", get(google::begin))
        .route("/links/{id}/google/delete", post(google::unlink))
        .route("/google/callback", get(google::callback))
        .route("/schools", get(web::schools_page))
        .route("/language/{code}", get(web::set_language))
        .route("/privacy", get(web::privacy))
        .route("/account/export.json", get(web::export))
        .route("/account/delete", post(web::delete_account))
        .route("/security", get(web::security_page))
        .route("/security/enrol/start", post(web::enrol_start))
        .route("/security/enrol/finish", post(web::enrol_finish))
        .route("/security/{id}/forget", post(web::forget_key))
        .route("/mfa/challenge", post(web::mfa_challenge))
        .route("/mfa/verify", post(web::mfa_verify))
        .route("/admin", get(web::admin_page))
        .route("/admin/health", get(web::health_page))
        .route("/admin/{who}/{what}", post(web::admin_decide))
        .route("/cal/{file}", get(feed::serve))
        .route("/healthz", get(healthz))
        .layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    tracing::info!("listening on {listen}");

    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(quit())
        .await?;
    scheduler.abort();
    Ok(())
}

async fn healthz(axum::extract::State(state): axum::extract::State<AppState>) -> &'static str {
    match sqlx::query("select 1").execute(&state.pool).await {
        Ok(_) => "ok",
        Err(_) => "database unreachable",
    }
}

async fn quit() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
