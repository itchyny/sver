use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(author, version, about = "Version calculator based on source code.", long_about = None)]
pub(crate) struct Args {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// calc version
    Calc {
        /// target paths
        paths: Vec<String>,

        /// format of calculation result
        #[arg(short, long, default_value = "version-only")]
        output: OutputFormat,
        /// length of version
        #[arg(short, long, default_value = "short")]
        length: VersionLength,
    },
    /// list package dependencies
    List {
        /// target path
        #[arg(default_value = ".")]
        path: String,
    },

    /// generate empty config file
    Init {
        /// target path
        #[arg(default_value = ".")]
        path: String,
    },

    /// validate all config files in repository
    Validate,
}

#[derive(Debug, Clone, ValueEnum)]
pub(crate) enum OutputFormat {
    VersionOnly,
    Toml,
    Json,
}

#[derive(Debug, Clone, ValueEnum)]
pub(crate) enum VersionLength {
    Short,
    Long,
}
