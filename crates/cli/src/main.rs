//! The `mymcps` binary: the server, and the commands an operator runs on it.

mod reset_password;
mod server;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use mymcps_core::{Config, Core, Db};
use mymcps_upstream::Upstream;
use mymcps_web::AppState;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Parser)]
#[command(name = "mymcps", version, about = "Self-hosted MCP gateway")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Migrate the database, then serve HTTP on $HOST:$PORT (the default)
    Serve,
    /// Apply pending database migrations and exit
    #[command(name = "migration:run")]
    MigrationRun {
        /// Accepted for compatibility with the Node app, where production needed it
        #[arg(long)]
        force: bool,
    },
    /// Print a new APP_KEY
    #[command(name = "generate:key")]
    GenerateKey,
    /// Reset a user password using hidden prompts (requires server access)
    #[command(name = "user:reset-password")]
    UserResetPassword {
        /// Email address of the account to recover
        email: String,
    },
    /// Reload the Deno cache for npm MCPs that track latest
    #[command(name = "mcp:update")]
    McpUpdate,
    /// Exit successfully when the server on this machine answers its health check
    Healthcheck,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    restrict_new_files();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let outcome = runtime.block_on(async {
        match cli.command.unwrap_or(Command::Serve) {
            Command::Serve => serve().await,
            Command::MigrationRun { .. } => migrate().await,
            Command::GenerateKey => {
                println!("{}", mymcps_core::config::generate_app_key());
                Ok(ExitCode::SUCCESS)
            }
            Command::UserResetPassword { email } => user_reset_password(&email).await,
            Command::McpUpdate => mcp_update().await,
            Command::Healthcheck => healthcheck().await,
        }
    });

    match outcome {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Everything written from here on is private to the user the server runs
/// as: the generated key, the SQLite database with its encrypted secrets,
/// and the MCP sandboxes. The Deno children inherit this.
fn restrict_new_files() {
    #[cfg(unix)]
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
}

fn init_logging(config: &Config) {
    let level = match config.log_level.to_ascii_lowercase().as_str() {
        "trace" => "trace",
        "debug" => "debug",
        "warn" => "warn",
        "error" | "fatal" => "error",
        _ => "info",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("RUST_LOG").unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(format!("{level},sqlx=warn,hyper=warn"))
    });
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stdout);
    // One JSON object per line in production, for log collectors.
    if config.is_production() {
        builder.json().init();
    } else {
        builder.init();
    }
}

type CommandResult = Result<ExitCode, Box<dyn std::error::Error + Send + Sync>>;

/// How often the files agents uploaded for built-in MCPs are checked for
/// expiry.
const UPLOAD_SWEEP_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The directories npm MCPs run in and Deno caches packages in. The server
/// creates them before the first npm MCP needs one, so that a volume
/// mounted empty has them with the server's own permissions.
async fn create_data_directories(config: &Config) -> std::io::Result<()> {
    for name in ["mcp-sandboxes", "deno-cache"] {
        tokio::fs::create_dir_all(config.data_dir.join(name)).await?;
    }
    Ok(())
}

async fn serve() -> CommandResult {
    let config = Config::from_env()?;
    init_logging(&config);
    let address = (config.host.clone(), config.port);

    let core = Core::boot(config).await?;
    create_data_directories(&core.config)
        .await
        .map_err(|error| format!("Cannot create the data directories: {error}"))?;
    let state = AppState::new(core.clone());
    // Deletes the expired uploads, including the ones a previous run left.
    let upload_sweeper =
        Arc::new(state.upstream.builtin_env().uploads.clone()).start_sweeper(UPLOAD_SWEEP_INTERVAL);
    let listener = TcpListener::bind(&address)
        .await
        .map_err(|error| format!("Cannot listen on {}:{}: {error}", address.0, address.1))?;
    tracing::info!(host = %address.0, port = address.1, version = mymcps_core::VERSION, "started HTTP server");

    // Refreshes the npm MCPs that follow `latest`, when the instance asks for it.
    let mcp_gateway = state.mcp_gateway.clone();
    mcp_gateway.auto_update.start().await?;

    let service = mymcps_web::app::service(state);
    server::serve(
        listener,
        service,
        server::Limits::default(),
        shutdown_signal(),
    )
    .await;

    mcp_gateway.auto_update.stop();
    upload_sweeper.abort();
    core.db.close().await;
    Ok(ExitCode::SUCCESS)
}

/// Resolves on SIGTERM, which a container runtime sends, or on Ctrl-C.
async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::info!("shutting down");
}

async fn migrate() -> CommandResult {
    let config = Config::from_env()?;
    let db = Db::open(&config.database_path()).await?;
    let ran = db.migrate().await?;
    for name in &ran {
        println!("migrated {name}");
    }
    if ran.is_empty() {
        println!("Already up to date");
    }
    db.close().await;
    Ok(ExitCode::SUCCESS)
}

async fn user_reset_password(email: &str) -> CommandResult {
    let config = Config::from_env()?;
    let core = Core::boot(config).await?;
    let outcome = reset_password::reset(&core, email, |label| {
        rpassword::prompt_password(format!("{label}: "))
    })
    .await;
    core.db.close().await;
    println!("{}", outcome?);
    Ok(ExitCode::SUCCESS)
}

/// Reload the packages of the npm MCPs that follow `latest`, test each one,
/// and fail when one could not be updated.
async fn mcp_update() -> CommandResult {
    let config = Config::from_env()?;
    init_logging(&config);
    let core = Core::boot(config).await?;
    create_data_directories(&core.config)
        .await
        .map_err(|error| format!("Cannot create the data directories: {error}"))?;
    let upstream = Upstream::new(core.clone(), mymcps_web::state::builtins());
    let outcome = upstream.update_latest_tracking_mcps().await;
    core.db.close().await;

    let result = outcome?;
    println!(
        "Updated {} npm MCP(s); skipped {} pinned MCP(s)",
        result.updated, result.skipped
    );
    for failure in &result.failed {
        eprintln!("{}: {}", failure.slug, failure.error);
    }
    Ok(if result.failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Liveness check for the container: no curl or wget needed in the image.
async fn healthcheck() -> CommandResult {
    let port = std::env::var("PORT")
        .ok()
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap_or(3333);
    // `HOST=localhost` may listen on the IPv6 loopback only.
    let mut last_error: Box<dyn std::error::Error + Send + Sync> =
        "The server did not answer its health check with 200".into();
    for address in ["127.0.0.1", "::1"] {
        let check = async {
            let mut stream = TcpStream::connect((address, port)).await?;
            stream
                .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .await?;
            let mut response = Vec::new();
            stream.read_to_end(&mut response).await?;
            Ok::<_, std::io::Error>(response)
        };
        match tokio::time::timeout(Duration::from_secs(2), check).await {
            Ok(Ok(response)) if response.starts_with(b"HTTP/1.1 200") => {
                return Ok(ExitCode::SUCCESS);
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => last_error = error.into(),
            Err(_) => last_error = "The health check timed out".into(),
        }
    }
    Err(last_error)
}
