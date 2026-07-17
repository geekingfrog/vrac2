#![allow(unused_imports)]
use scrypt::password_hash::PasswordHasher;
use scrypt::{phc::PasswordHash, Scrypt};
use vrac::handlers::gen::{GenTokenForm, StorageBackendType};

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> BoxResult<()> {
    let password = "hunter2";
    let hash: PasswordHash = Scrypt::default().hash_password(password.as_bytes())?;
    println!("hash: {:?}", hash.to_string());
    Ok(())
}
