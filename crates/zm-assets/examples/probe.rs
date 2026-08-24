use zm_assets::{AssetManager, OfficialAssetManager};
use zm_core::GameKind;

#[tokio::main]
async fn main() {
    let cache = std::env::temp_dir().join("zm-linux-probe");
    let manager = OfficialAssetManager::new(cache).expect("create asset manager");
    let download = std::env::args().any(|arg| arg == "--download");
    for game in [GameKind::Zm4, GameKind::Zm5] {
        match manager.resolve_version(game).await {
            Ok(version) => println!("{}: {} {}", game.slug(), version.file_name, version.swf_url),
            Err(error) => {
                eprintln!("{}: {error}", game.slug());
                std::process::exit(1);
            }
        }
        if download {
            match manager.ensure_game(game).await {
                Ok(asset) => println!(
                    "{}: cached={} sha256={} path={}",
                    game.slug(),
                    asset.cache_hit,
                    asset.sha256,
                    asset.path.display()
                ),
                Err(error) => {
                    eprintln!("{} download: {error}", game.slug());
                    std::process::exit(1);
                }
            }
        }
    }
}
