use clap::{Parser, Subcommand};
use password_hash::rand_core::OsRng;
use password_hash::SaltString;
use rpassword;
use scrypt::password_hash::PasswordHasher;
use scrypt::Scrypt;
use std::error::Error;
use vrac::db::DBService;

type BoxResult<T> = Result<T, Box<dyn Error>>;

#[derive(Parser)]
struct Cli {
    #[arg(long, default_value = "./test.sqlite")]
    sqlite_path: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    AddUser { username: String },
    ChangePassword { username: String },
}

#[tokio::main]
async fn main() -> BoxResult<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt::init();
    match cli.command {
        Command::AddUser { username } => add_user(&cli.sqlite_path, &username).await,
        Command::ChangePassword { username } => change_password(&cli.sqlite_path, &username).await,
    }
}

async fn add_user(sqlite_path: &str, username: &str) -> BoxResult<()> {
    println!("Adding user with username {username}");
    let db = DBService::new(sqlite_path).await?;
    let password = rpassword::prompt_password("Input password: ")?;
    let phc = hash(&password)?;
    db.create_account(username, &phc).await?;
    // need to close the pool to force a full flush/fsync
    db.close().await;
    println!("User created");
    Ok(())
}

async fn change_password(sqlite_path: &str, username: &str) -> BoxResult<()> {
    println!("Changing password for username {username}");
    let db = DBService::new(sqlite_path).await?;
    let password = rpassword::prompt_password("Input new password: ")?;
    let phc = hash(&password)?;
    db.change_password(username, &phc).await?;
    // need to close the pool to force a full flush/fsync
    db.close().await;
    println!("password updated");
    Ok(())
}

fn hash(password: &str) -> BoxResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Scrypt
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}
