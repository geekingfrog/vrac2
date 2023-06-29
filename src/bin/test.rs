#![allow(unused_imports)]
use vrac::handlers::gen::{GenTokenForm, StorageBackendType};

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> BoxResult<()> {
    let s = serde_json::to_string(&StorageBackendType::LocalFS)?;
    println!("{}", s);
    Ok(())
}
