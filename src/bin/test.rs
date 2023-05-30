type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> BoxResult<()> {
    Ok(())
}
