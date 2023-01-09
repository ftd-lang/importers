#[macro_use]
extern crate clap;
extern crate log;

use clap::{ArgMatches, Command};

use std::env;
use std::path::PathBuf;

mod cmd;

const VERSION: &str = concat!("v", crate_version!());

fn main() {


    let command = create_clap_command();

    // Check which subcommand the user ran...
    match command.get_matches().subcommand() {
        Some(("build", sub_matches)) => cmd::build::execute(sub_matches),
        _ => unreachable!(),
    };

}

/// Create a list of valid arguments and sub-commands
fn create_clap_command() -> Command {
    Command::new(crate_name!())
        .about(crate_description!())
        .version(VERSION)
        .propagate_version(true)
        .arg_required_else_help(true)
        .after_help(
            "For more information about a specific command, try `mdbook <command> --help`\n\
             The source code for mdBook is available at: https://github.com/rust-lang/mdBook",
        )
        .subcommand(cmd::build::make_subcommand())

}

fn get_book_dir(args: &ArgMatches) -> PathBuf {
    if let Some(p) = args.get_one::<PathBuf>("dir") {
        // Check if path is relative from current dir, or absolute...
        if p.is_relative() {
            env::current_dir().unwrap().join(p)
        } else {
            p.to_path_buf()
        }
    } else {
        env::current_dir().expect("Unable to determine the current directory")
    }
}
