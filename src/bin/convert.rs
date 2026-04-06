//! `eruditio-convert` — CLI ebook converter, inspired by calibre's `ebook-convert`.
//!
//! Usage:
//!     eruditio-convert input.epub output.mobi [options]
//!     eruditio-convert --list-formats

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use eruditio::{ConversionOptions, Format, Metadata, Pipeline};

/// Convert ebooks between formats.
///
/// Reads an ebook in one format and writes it in another, applying optional
/// transforms and metadata overrides. Formats are auto-detected from file
/// extensions.
#[derive(Parser)]
#[command(name = "eruditio-convert", version, about, long_about = None)]
struct Cli {
    /// Input ebook file path.
    #[arg(required_unless_present = "list_formats")]
    input: Option<PathBuf>,

    /// Output ebook file path (format determined by extension).
    #[arg(required_unless_present = "list_formats")]
    output: Option<PathBuf>,

    // ── Metadata overrides ──────────────────────────────────────────
    /// Set the book title.
    #[arg(long)]
    title: Option<String>,

    /// Set the book authors (comma-separated).
    #[arg(long)]
    authors: Option<String>,

    /// Set the publisher.
    #[arg(long)]
    publisher: Option<String>,

    /// Set the language (e.g. "en", "zh").
    #[arg(long)]
    language: Option<String>,

    /// Set the ISBN.
    #[arg(long)]
    isbn: Option<String>,

    /// Set the book description.
    #[arg(long)]
    description: Option<String>,

    /// Set the series name.
    #[arg(long)]
    series: Option<String>,

    /// Set the series index (e.g. 1, 2.5).
    #[arg(long)]
    series_index: Option<f64>,

    /// Set tags / subjects (comma-separated).
    #[arg(long)]
    tags: Option<String>,

    /// Set the rights / license string.
    #[arg(long)]
    rights: Option<String>,

    // ── Transform control ───────────────────────────────────────────
    /// Disable all transforms (pass-through conversion).
    #[arg(long)]
    no_transforms: bool,

    /// Disable HTML normalization.
    #[arg(long)]
    no_html_normalize: bool,

    /// Disable chapter structure detection.
    #[arg(long)]
    no_structure_detect: bool,

    /// Disable table-of-contents generation.
    #[arg(long)]
    no_toc_generate: bool,

    /// Disable removal of unreferenced manifest items.
    #[arg(long)]
    no_trim_manifest: bool,

    /// Disable cover image detection.
    #[arg(long)]
    no_cover_detect: bool,

    /// Disable data URI image extraction.
    #[arg(long)]
    no_data_uri_extract: bool,

    // ── Output control ──────────────────────────────────────────────
    /// Print supported input and output formats, then exit.
    #[arg(long)]
    list_formats: bool,

    /// Show detailed progress information.
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> ExitCode {
    let mut cli = Cli::parse();

    if cli.list_formats {
        print_formats();
        return ExitCode::SUCCESS;
    }

    // Safe: clap enforces these are present when list_formats is absent.
    let input_path = cli.input.take().unwrap();
    let output_path = cli.output.take().unwrap();

    if let Err(e) = run_conversion(&cli, &input_path, &output_path) {
        eprintln!("Error: {e}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn run_conversion(cli: &Cli, input_path: &PathBuf, output_path: &PathBuf) -> Result<(), String> {
    // Detect formats from file extensions.
    let in_ext = input_path
        .extension()
        .and_then(|e| e.to_str())
        .ok_or_else(|| format!("Cannot determine format: {}", input_path.display()))?;

    let out_ext = output_path
        .extension()
        .and_then(|e| e.to_str())
        .ok_or_else(|| format!("Cannot determine format: {}", output_path.display()))?;

    let in_fmt = Format::from_extension(in_ext)
        .ok_or_else(|| format!("Unsupported input format: .{in_ext}"))?;

    let out_fmt = Format::from_extension(out_ext)
        .ok_or_else(|| format!("Unsupported output format: .{out_ext}"))?;

    if cli.verbose {
        eprintln!(
            "Converting {} ({}) -> {} ({})",
            input_path.display(),
            in_fmt,
            output_path.display(),
            out_fmt
        );
    }

    // Build conversion options.
    let options = build_options(cli);

    // Open files.
    let in_file =
        File::open(input_path).map_err(|e| format!("Cannot open {}: {e}", input_path.display()))?;
    let mut reader = BufReader::new(in_file);

    let out_file = File::create(output_path)
        .map_err(|e| format!("Cannot create {}: {e}", output_path.display()))?;
    let mut writer = BufWriter::new(out_file);

    // Run the pipeline.
    let start = Instant::now();
    let pipeline = Pipeline::new();

    let book = pipeline
        .convert(in_fmt, out_fmt, &mut reader, &mut writer, &options)
        .map_err(|e| format!("Conversion failed: {e}"))?;

    let elapsed = start.elapsed();

    if cli.verbose {
        let title = book.metadata.title.as_deref().unwrap_or("(untitled)");
        let authors = if book.metadata.authors.is_empty() {
            "(unknown)".to_string()
        } else {
            book.metadata.authors.join(", ")
        };
        let chapters = book.spine.items.len();
        eprintln!("Title:    {title}");
        eprintln!("Authors:  {authors}");
        eprintln!("Chapters: {chapters}");
        eprintln!("Time:     {elapsed:.2?}");
    }

    eprintln!("Output written to {}", output_path.display());

    Ok(())
}

fn build_options(cli: &Cli) -> ConversionOptions {
    let mut opts = if cli.no_transforms {
        ConversionOptions::none()
    } else {
        ConversionOptions::all()
    };

    // Individual transform overrides (only meaningful when not --no-transforms).
    if !cli.no_transforms {
        if cli.no_html_normalize {
            opts.normalize_html = false;
        }
        if cli.no_structure_detect {
            opts.detect_structure = false;
        }
        if cli.no_toc_generate {
            opts.generate_toc = false;
        }
        if cli.no_trim_manifest {
            opts.trim_manifest = false;
        }
        if cli.no_cover_detect {
            opts.detect_cover = false;
        }
        if cli.no_data_uri_extract {
            opts.extract_data_uris = false;
        }
    }

    // Metadata overrides.
    if has_metadata_overrides(cli) {
        let mut meta = Metadata::default();

        if let Some(ref t) = cli.title {
            meta.title = Some(t.clone());
        }
        if let Some(ref a) = cli.authors {
            meta.authors = a.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Some(ref p) = cli.publisher {
            meta.publisher = Some(p.clone());
        }
        if let Some(ref l) = cli.language {
            meta.language = Some(l.clone());
        }
        if let Some(ref i) = cli.isbn {
            meta.isbn = Some(i.clone());
        }
        if let Some(ref d) = cli.description {
            meta.description = Some(d.clone());
        }
        if let Some(ref s) = cli.series {
            meta.series = Some(s.clone());
        }
        if let Some(idx) = cli.series_index {
            meta.series_index = Some(idx);
        }
        if let Some(ref t) = cli.tags {
            meta.subjects = t.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Some(ref r) = cli.rights {
            meta.rights = Some(r.clone());
        }

        opts = opts.with_metadata(meta);
    }

    opts
}

fn has_metadata_overrides(cli: &Cli) -> bool {
    cli.title.is_some()
        || cli.authors.is_some()
        || cli.publisher.is_some()
        || cli.language.is_some()
        || cli.isbn.is_some()
        || cli.description.is_some()
        || cli.series.is_some()
        || cli.series_index.is_some()
        || cli.tags.is_some()
        || cli.rights.is_some()
}

fn print_formats() {
    let pipeline = Pipeline::new();
    let registry = pipeline.registry();

    let readable = registry.readable_formats();
    let writable = registry.writable_formats();

    println!("Supported input formats:");
    let mut names: Vec<&str> = readable.iter().map(|f| f.name()).collect();
    names.sort_unstable();
    for name in &names {
        println!("  {name}");
    }

    println!();
    println!("Supported output formats:");
    let mut names: Vec<&str> = writable.iter().map(|f| f.name()).collect();
    names.sort_unstable();
    for name in &names {
        println!("  {name}");
    }
}
