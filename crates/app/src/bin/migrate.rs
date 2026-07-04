//! Applies pending DB migrations and exits. Invoked by Fly's `release_command`
//! so migrations run once per deploy, before the new version serves traffic.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    platform::observability::init_tracing("info");
    let settings = platform::config::Settings::load()?;
    let pool = platform::db::make_pool(&settings.database).await?;
    platform::db::run_migrations(&pool).await?;
    tracing::info!("migrations applied");
    Ok(())
}
