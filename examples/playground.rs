#[allow(unused_imports)]
use loco_rs::{cli::playground, prelude::*};
use yorishiro_core::app::App;

#[tokio::main]
async fn main() -> loco_rs::Result<()> {
    let _ctx = playground::<App>().await?;

    println!("welcome to playground. edit me at `examples/playground.rs`");

    Ok(())
}
