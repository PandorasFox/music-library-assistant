mod menu;
pub mod picker;

use crate::config::Config;
use anyhow::Result;

pub fn run_menu(config: Config) -> Result<()> {
    menu::run(config)
}
