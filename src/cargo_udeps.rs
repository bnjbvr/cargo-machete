use std::collections::{HashMap, HashSet};

use log::debug;

use crate::PackageAnalysis;

#[derive(serde::Deserialize)]
struct CargoUdepsPackage {
    normal: Vec<String>,
}

#[derive(serde::Deserialize)]
struct CargoUdepsOutput {
    success: bool,
    unused_deps: HashMap<String, CargoUdepsPackage>,
}

pub(crate) fn compare(our_analysis: &mut PackageAnalysis) -> anyhow::Result<()> {
    debug!("checking with cargo-udeps...");

    // Run cargo-udeps!
    let mut cmd = std::process::Command::new("cargo");
    cmd.args([
        "+nightly",
        "udeps",
        "--all-features",
        "--output",
        "json",
        "-p",
        &our_analysis.package_name,
    ]);
    let output = cmd.output()?;
    let output_str = String::from_utf8(output.stdout)?;
    let analysis: CargoUdepsOutput = serde_json::from_str(&output_str)?;

    if analysis.success {
        debug!("cargo-udeps didn't find any unused dependency");
        return Ok(());
    }

    let mut udeps_set = None;
    for (k, v) in analysis.unused_deps {
        if !k.starts_with(&our_analysis.package_name) {
            continue;
        }
        udeps_set = Some(v.normal.into_iter().collect::<HashSet<_>>());
    }

    if let Some(udeps_set) = udeps_set {
        let our_set: HashSet<String> = our_analysis.unused.iter().cloned().collect::<HashSet<_>>();
        let inter_set = our_set.intersection(&udeps_set);
        our_analysis.unused = inter_set.into_iter().cloned().collect();
    }

    Ok(())
}
