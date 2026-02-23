use crate::config::Config;

pub fn run_setup() -> Result<(), String> {
    match crate::tui::setup::run_setup_tui()? {
        true => {
            println!();
            println!(
                "  Configuration saved to {}",
                Config::config_path().display()
            );
            println!();
            println!("  You're all set! Try:");
            println!("    oobo sessions   — view your AI chat sessions");
            println!("    oobo dash       — check your configuration");
            println!("    oobo commit     — commit with context capture");
            println!();
            Ok(())
        }
        false => {
            println!();
            println!("  Setup cancelled.");
            println!();
            Ok(())
        }
    }
}
