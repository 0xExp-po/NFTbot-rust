mod config;
mod errors;
mod modules;
mod programs;
mod tpu_client;
mod utils;

use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Select};
use log::error;

#[tokio::main]
async fn main() {
    std::env::set_var("RUST_LOG", "info");
    env_logger::builder()
        .format_module_path(false)
        .format_timestamp_millis()
        .init();

    if let Err(e) = init().await {
        error!("{}", e);
    }
}

async fn init() -> Result<()> {
    print_welcome()?;

    let (settings, wallets) = config::read_settings()?;

    let option = loop {
        if let Some(index) = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Select an operation")
            .item("Start candy machine tasks")
            .item("Start magic eden launchpad tasks")
            .item("Start taiyo tasks")
            .item("Start temple tasks")
            .default(0)
            .interact_opt()?
        {
            break index;
        }
    };

    match option {
        // start candy machine tasks
        0 => {
            modules::candy_machine::instance::Instance::new(settings, wallets)
                .await?
                .init()
                .await?
        }
        1 => {
            modules::magic_eden_launchpad::instance::Instance::new(settings, wallets)
                .await?
                .init()
                .await?
        }
        2 => {
            modules::solport::instance::Instance::new(settings, wallets)
                .await?
                .init()
                .await?
        }
        3 => {
            modules::temple::instance::Instance::new(settings, wallets)
                .await?
                .init()
                .await?
        }
        _ => (),
    }

    Ok(())
}

fn print_welcome() -> Result<()> {
    viuer::print_from_file("src/anya.jpg", &viuer::Config::default())?;
    println!("");
    Ok(())
}
