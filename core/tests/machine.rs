//! Machine tests: real tools, GPU and Topaz on a local PC. Ignored in CI; run with
//! `UPSCALE_TOOLS_DIR=<tools folder> cargo test -p upscale-core --test machine -- --ignored`.

use upscale_core::gpu::{list_gpus, pick_gpu};
use upscale_core::log::MemoryLog;
use upscale_core::process::SystemRunner;
use upscale_core::settings::DEFAULT_TOPAZ_FILTER;
use upscale_core::tools::{default_tools_dir, find_scripts_dir, mlrt, set, topaz};

#[test]
#[ignore = "needs VapourSynth, vs-mlrt and a GPU"]
fn vs_mlrt_selftest() {
    let runner = SystemRunner;
    let gpu = pick_gpu(&list_gpus(&runner));
    let tools_dir = default_tools_dir();
    let (rows, _) = set::inspect(&runner, &tools_dir, &gpu, None);
    let path = |name: &str| {
        rows.iter()
            .find(|r| r.name == name && r.ok)
            .and_then(|r| r.path.clone())
    };
    let vspipe = path("VapourSynth").expect("VapourSynth R79 with plugins");
    let mlrt_dir = path("vs-mlrt").expect("vs-mlrt in the tools folder");
    let vpy = find_scripts_dir()
        .expect("resources/scripts")
        .join("upscale.vpy");
    let log = MemoryLog::default();
    mlrt::selftest(
        &runner,
        &vspipe,
        &vpy,
        gpu.default_backend(),
        &mlrt_dir,
        &tools_dir.join("vs-mlrt"),
        &log,
    )
    .unwrap();
}

#[test]
#[ignore = "needs Topaz Video AI 7.x, signed in"]
fn topaz_selftest() {
    let runner = SystemRunner;
    let dir = upscale_core::settings::Settings::default()
        .topaz_dir()
        .expect("ProgramFiles");
    let log = MemoryLog::default();
    let t = topaz::init_topaz(&runner, &dir, &log).unwrap();
    topaz::selftest(&runner, &t, DEFAULT_TOPAZ_FILTER, &log).unwrap();
}
