//! Minimal CLI over `gel`. Two subcommands:
//!
//! * `filter-svg` - the artifact-stripping step `gel-mcp`'s `filter_svg` tool
//!   wraps; see gel-lib/src/svg_filter.rs for why it exists.
//! * `run` - executes a layout program (a JSON array of instructions) against
//!   an SVG. This is the headless entry point the layout service uses: no MCP
//!   server, no JSON-RPC round trip per shape.
//!
//! Plain arg parsing (no clap) while the command set stays this small.

use std::process::ExitCode;

fn usage() -> String {
    "Usage:\n  \
    gel-cli filter-svg <in.svg> <out.svg> [attr=value ...]\n  \
    gel-cli run [--debug] <program.json> <in.svg>\n\n\
    filter-svg strips any element (and its subtree) matching one of the given\n\
    attr=value rules, or LibreOffice's class=\"BoundingBox\" helper\n\
    rectangles by default if no rules are given.\n\n\
    run executes a layout program against an SVG and reports the groups it\n\
    produced. --debug makes a failed Assert stop the run; without it, asserts\n\
    are collected and printed as warnings, which is how it behaves in\n\
    production.\n\n\
    Job metadata is not supplied here. Prepend it as a RunCode, the way the\n\
    quote generator will:\n      \
    jq -s 'add' vars.json library.json layout.json > composed.json"
        .into()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("filter-svg") => filter_svg(&args[1..]),
        Some("run") => run(&args[1..]),
        _ => {
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> ExitCode {
    let debug = args.iter().any(|a| a == "--debug");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let (Some(program_path), Some(svg_path)) = (positional.first(), positional.get(1)) else {
        eprintln!("{}", usage());
        return ExitCode::FAILURE;
    };
    let (program_path, svg_path) = (*program_path, *svg_path);

    let program_json = match std::fs::read_to_string(program_path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("could not read {program_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let program: Vec<gel::Instruction> = match serde_json::from_str(&program_json) {
        Ok(program) => program,
        Err(e) => {
            eprintln!("could not parse {program_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut data = gel::Data::from_svg_path_with_style(std::path::Path::new(svg_path));
    data.debug = debug;

    println!(
        "running {} instruction(s) over {svg_path}{}",
        program.len(),
        if debug { " [debug]" } else { "" }
    );
    let outcome = data.query(program);

    // Warnings are printed even when the run failed - an assert that fired
    // before the failure is usually the thing that explains it.
    let warnings = data.warnings.lock().unwrap().clone();
    if !warnings.is_empty() {
        eprintln!("\n{} warning(s):", warnings.len());
        for warning in &warnings {
            match &warning.group {
                Some(group) => {
                    eprintln!("  [{}] {} (in group '{group}')", warning.id, warning.message)
                }
                None => eprintln!("  [{}] {}", warning.id, warning.message),
            }
        }
        eprintln!();
    }

    for (name, count, elapsed) in gel::profile_report() {
        eprintln!(
            "  {name:<16} {count:>6} calls  {:>8.2}s  {:>7.2}ms each",
            elapsed.as_secs_f64(),
            elapsed.as_secs_f64() * 1000.0 / count as f64,
        );
    }

    if let Err(e) = outcome {
        eprintln!("program failed: {e}");
        return ExitCode::FAILURE;
    }

    // Until `Emit` exists, listing the groups is how a run gets inspected.
    let groups = data.groups.lock().unwrap();
    let mut names: Vec<&String> = groups.keys().collect();
    names.sort();
    for name in names {
        let entries = &groups[name];
        let shapes: usize = entries.iter().map(|entry| entry.len()).sum();
        println!("  {name}: {} entr(ies), {shapes} shape(s)", entries.len());
    }

    ExitCode::SUCCESS
}

fn filter_svg(args: &[String]) -> ExitCode {
    let Some(in_path) = args.first() else {
        eprintln!("{}", usage());
        return ExitCode::FAILURE;
    };
    let Some(out_path) = args.get(1) else {
        eprintln!("{}", usage());
        return ExitCode::FAILURE;
    };

    let rules: Vec<gel::StripRule> = if args.len() > 2 {
        args[2..]
            .iter()
            .filter_map(|pair| {
                let (attr, value) = pair.split_once('=')?;
                Some(gel::StripRule::new(attr, value))
            })
            .collect()
    } else {
        gel::known_artifact_rules()
    };

    let svg = match std::fs::read_to_string(in_path) {
        Ok(svg) => svg,
        Err(e) => {
            eprintln!("could not read {in_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let result = match gel::strip_elements(&svg, &rules) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("filter failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = std::fs::write(out_path, &result.svg) {
        eprintln!("could not write {out_path}: {e}");
        return ExitCode::FAILURE;
    }

    println!("wrote {out_path} ({} elements stripped)", result.elements_stripped);
    ExitCode::SUCCESS
}
