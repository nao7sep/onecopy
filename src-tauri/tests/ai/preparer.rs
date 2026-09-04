//! Test-only managed-artifact preparer. It deliberately calls the production
//! registry and acquisition path; URLs, pins, platform rules, and publication
//! logic are never copied into JavaScript test infrastructure.

use onecopy_lib::ai_dependencies::{self, Requirement};
use onecopy_lib::binaries::BinaryStatus;
use onecopy_lib::binaries_manager;
use serde_json::json;

fn usage() -> ! {
    eprintln!("usage: onecopy-ai-preparer <prepare|verify> ROOT REQUIREMENT...");
    std::process::exit(2);
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let action = args.next().unwrap_or_else(|| usage());
    let root = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| usage());
    if action != "prepare" && action != "verify" {
        usage();
    }
    let requirements = args
        .map(|value| value.parse::<Requirement>())
        .collect::<Result<Vec<_>, _>>()?;
    if requirements.is_empty() {
        return Err("at least one AI dependency requirement is required".to_string());
    }
    if action == "prepare" {
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        binaries_manager::reset_temp_dir(&root);
        for id in ai_dependencies::dependency_ids(&requirements) {
            let spec = binaries_manager::spec_of(id)
                .ok_or_else(|| format!("dependency is not available on this platform: {id}"))?;
            if binaries_manager::state_of(&root, spec).status != BinaryStatus::UpToDate {
                binaries_manager::install_entry(&root, id, |progress| {
                    println!(
                        "{}",
                        json!({ "event": "dependency-progress", "id": id, "progress": progress })
                    );
                })?;
            }
        }
    } else {
        require_offline_mode()?;
    }
    let context = ai_dependencies::require_prepared(&root, &requirements)?;
    println!(
        "{}",
        json!({ "event": "prepared-context", "context": context })
    );
    Ok(())
}

fn require_offline_mode() -> Result<(), String> {
    if std::env::var("ONECOPY_AI_OFFLINE").as_deref() == Ok("1") {
        Ok(())
    } else {
        Err("AI verification and execution require ONECOPY_AI_OFFLINE=1".to_string())
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
