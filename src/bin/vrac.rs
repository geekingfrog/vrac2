use std::env;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;

use axum::Router;
use base64::Engine;
use clap::{Parser, Subcommand};
use hyper::{Body, Request};
use hyper_tls::HttpsConnector;
use mpart_async::client::MultipartRequest;
use vrac::handlers::gen::{GenTokenForm, StorageBackendType};
use vrac::{app::build, state::AppState};

type BoxResult<T> = Result<T, axum::BoxError>;

#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Serve {
        #[arg(long, default_value = "./test.sqlite")]
        sqlite_path: String,

        #[arg(long, default_value = "/tmp/vrac/")]
        storage_path: String,

        #[arg(long, default_value_t = 8000)]
        port: u16,

        #[arg(long, default_value = "127.0.0.1")]
        bind_address: String,
    },
    Upload {
        path: PathBuf,

        #[arg(long, default_value = "https://vrac.geekingfrog.com")]
        base_url: String,

        #[arg(long)]
        name: Option<String>,

        #[arg(long, default_value_t = 48)]
        expires_hours: i64,

        #[arg(short, long, default_value_t = false)]
        no_expires: bool,
    },
}

#[tokio::main]
async fn main() -> BoxResult<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Command::Serve {
            sqlite_path,
            storage_path,
            port,
            bind_address,
        } => serve(sqlite_path, storage_path, port, bind_address).await,
        Command::Upload {
            path,
            base_url,
            name,
            expires_hours,
            no_expires,
        } => upload(path, base_url, name, expires_hours, no_expires).await,
    }
}

async fn serve(
    sqlite_path: String,
    storage_path: String,
    port: u16,
    bind_address: String,
) -> BoxResult<()> {
    tracing::info!("Local fs for storage at {}", storage_path);
    tokio::fs::create_dir_all(&storage_path).await?;

    tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&sqlite_path)
        .await?;

    let state = AppState::new("templates/**/*.html", &sqlite_path, &storage_path).await?;
    state.db.migrate().await?;

    let addr = IpAddr::from_str(&bind_address)?;
    let addr = SocketAddr::from((addr, port));
    let app = build(state.clone());

    tokio::try_join!(
        webserver(addr, app),
        background_cleanup(&state.db, &state.storage_fs, &state.garage)
    )?;

    Ok(())
}

async fn webserver(addr: SocketAddr, app: Router) -> BoxResult<()> {
    tracing::info!("Listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

async fn background_cleanup(
    db: &vrac::db::DBService,
    storage_fs: &vrac::upload::LocalFsUploader,
    garage: &vrac::upload::GarageUploader,
) -> Result<(), axum::BoxError> {
    loop {
        vrac::cleanup::cleanup(&db, &storage_fs, &garage).await?;
        tokio::time::sleep(std::time::Duration::from_secs(60 * 5)).await;
    }
}

async fn upload(
    path: PathBuf,
    base_url: String,
    name: Option<String>,
    expires_hours: i64,
    no_expires: bool,
) -> BoxResult<()> {
    let base_url = url::Url::parse(&base_url)?;

    let https = HttpsConnector::new();
    let client = hyper::Client::builder().build::<_, hyper::Body>(https);

    let mut gen_url = base_url.clone();
    gen_url.set_path("/gen");

    let username =
        env::var("VRAC_USERNAME").map_err(|e| format!("VRAC_USERNAME not found {e:?}"))?;
    let password =
        env::var("VRAC_PASSWORD").map_err(|e| format!("VRAC_PASSWORD not found {e:?}"))?;

    let raw_auth = format!("{}:{}", username, password);
    let encoded_auth = base64::engine::general_purpose::STANDARD_NO_PAD.encode(raw_auth.as_bytes());

    let filename = name
        .or_else(|| path.file_name().map(|s| s.to_string_lossy().into_owned()))
        .ok_or_else(|| "Cannot get filename")?;

    let content_expires_after_hours = if no_expires {
        None
    } else {
        Some(expires_hours)
    };

    let form = GenTokenForm {
        path: filename,
        max_size_mib: None,
        content_expires_after_hours,
        token_valid_for_hour: 1,
        storage_backend: StorageBackendType::LocalFS,
    };

    let request = Request::post(hyper::Uri::from_str(gen_url.as_str()).unwrap())
        .header(
            hyper::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .header(
            hyper::header::AUTHORIZATION,
            format!("Basic {}", encoded_auth),
        )
        .body(serde_urlencoded::to_string(&form)?.into())?;

    let response = client.request(request).await?;
    let status_code = response.status();
    if !status_code.is_redirection() {
        return Err(format!("Couldn't create token, got status code: {}", status_code).into());
    }

    let location = response
        .headers()
        .get(hyper::header::LOCATION)
        .ok_or("No location returned")?;

    let mut upload_url = base_url.clone();
    upload_url.set_path(location.to_str()?);

    let mut mparts = MultipartRequest::default();
    mparts.add_file("file_1", path);

    let request = Request::post(hyper::Uri::from_str(upload_url.as_str()).unwrap())
        .header(
            hyper::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={}", mparts.get_boundary()),
        )
        .body(Body::wrap_stream(mparts))?;

    let response = client.request(request).await?;

    let status = response.status();
    if !status.is_redirection() {
        let body = hyper::body::to_bytes(response).await?;
        let strbody = String::from_utf8(body.to_vec())?;
        return Err(format!("Couldn't upload files {}\n{}", status, strbody).into());
    }

    // output the final url as a result
    println!("{}", upload_url);
    Ok(())
}
