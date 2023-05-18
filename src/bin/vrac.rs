use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;

use axum::Router;
use base64::Engine;
use clap::{Parser, Subcommand};
use mpart_async::client::{ByteStream, MultipartRequest};
use vrac::handlers::gen::GenTokenForm;
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
        Command::Upload { path, base_url } => upload(path, base_url).await,
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
        background_cleanup(&state.db, &state.storage_fs)
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
) -> Result<(), axum::BoxError> {
    loop {
        vrac::cleanup::cleanup(&db, &storage_fs).await?;
        tokio::time::sleep(std::time::Duration::from_secs(60 * 5)).await;
    }
}

async fn upload(path: PathBuf, base_url: String) -> BoxResult<()> {
    let client = hyper::Client::new();
    // let base_uri = Uri::from_static("http://localhost:8000");
    // let parts = base_uri.into_parts();

    let base_url = url::Url::parse("http://localhost:8000")?;
    let mut gen_url = base_url.clone();
    gen_url.set_path("/gen");

    let raw_auth = format!("{}:{}", "test", "testpassword");
    let encoded_auth = base64::engine::general_purpose::STANDARD_NO_PAD.encode(raw_auth.as_bytes());

    println!("{:?}", gen_url);
    println!("{}", gen_url);

    // let mut mparts: MultipartRequest<ByteStream> = MultipartRequest::default();
    // mparts.add_field("path", "todo-filename");
    // mparts.add_field("max-size-mib", "1024");
    // // default to 2 days for the content before expiration
    // mparts.add_field("content-expires", "48");
    // mparts.add_field("token-valid-for-hour", "1");

    let form = GenTokenForm {
        path: "coucou8".to_string(),
        max_size_mib: None,
        content_expires_after_hours: Some(48),
        token_valid_for_hour: 1,
    };

    // TODO: don't use multipart here but instead application/www-form-urlencoded
    let request = hyper::Request::post(hyper::Uri::from_str(gen_url.as_str()).unwrap())
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
    println!("got response status: {:?}", status_code);
    if status_code != hyper::StatusCode::SEE_OTHER {
        println!("oops");
        return Err(format!("Couldn't create token, got status code: {}", status_code).into());
    }

    Ok(())
}
