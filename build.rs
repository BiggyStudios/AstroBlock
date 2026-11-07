//! Compiles GLSL shaders in `shaders/` to SPIR-V at build time using naga
//! (pure Rust, so no Vulkan SDK / glslc is required to build the game).
//!
//! One source file can produce several variants via preprocessor defines.
//! Output goes to `$OUT_DIR/<name>.spv` and is embedded with `include_bytes!`.

use std::{env, fs, path::Path};

use naga::{
    ShaderStage,
    back::spv,
    front::glsl,
    valid::{Capabilities, ValidationFlags, Validator},
};

/// (source file, output name, defines)
const SHADERS: &[(&str, &str, &[&str])] = &[
    ("block.vert", "block.vert", &[]),
    ("block.frag", "block.frag", &[]),
    ("block.frag", "block_water.frag", &["WATER"]),
    ("block.frag", "block_far.frag", &["FAR"]),
    ("fullscreen.vert", "fullscreen.vert", &[]),
    ("sky.frag", "sky.frag", &[]),
    ("ui.vert", "ui.vert", &[]),
    ("ui.frag", "ui.frag", &[]),
    ("particles.vert", "particles.vert", &[]),
    ("particles.frag", "particles.frag", &[]),
    ("particles.frag", "particles_blend.frag", &["BLEND"]),
    ("particles_sim.comp", "particles_sim.comp", &[]),
    ("particles_emit.comp", "particles_emit.comp", &[]),
];

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    println!("cargo:rerun-if-changed=shaders");

    for &(source, output, defines) in SHADERS {
        let path = Path::new("shaders").join(source);
        println!("cargo:rerun-if-changed={}", path.display());
        let stage = match path.extension().and_then(|e| e.to_str()) {
            Some("vert") => ShaderStage::Vertex,
            Some("frag") => ShaderStage::Fragment,
            Some("comp") => ShaderStage::Compute,
            _ => panic!("unknown shader stage for {source}"),
        };
        compile(&path, output, stage, defines, Path::new(&out_dir));
    }
}

fn compile(path: &Path, output: &str, stage: ShaderStage, defines: &[&str], out_dir: &Path) {
    let source = resolve_includes(path);
    let name = format!("{} ({output})", path.display());

    let mut options = glsl::Options::from(stage);
    for &d in defines {
        options.defines.insert(d.to_string(), "1".to_string());
    }
    let module = glsl::Frontend::default()
        .parse(&options, &source)
        .unwrap_or_else(|e| panic!("\n{}", e.emit_to_string_with_path(&source, &name)));

    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .unwrap_or_else(|e| panic!("\n{}", e.emit_to_string_with_path(&source, &name)));

    let spv_options = spv::Options {
        // We handle Vulkan's Y-down clip space ourselves in the projection matrix,
        // so naga must NOT flip Y for us.
        flags: spv::WriterFlags::empty(),
        ..Default::default()
    };
    let pipeline_options = spv::PipelineOptions {
        shader_stage: stage,
        entry_point: "main".into(),
    };
    let words = spv::write_vec(&module, &info, &spv_options, Some(&pipeline_options))
        .unwrap_or_else(|e| panic!("{name}: SPIR-V generation failed: {e}"));

    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    fs::write(out_dir.join(format!("{output}.spv")), bytes).unwrap();
}

/// Replaces `#include "file"` lines with the file's contents (relative to the including
/// file). naga's GLSL frontend has no include support of its own.
fn resolve_includes(path: &Path) -> String {
    let source = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let dir = path.parent().unwrap();
    source
        .lines()
        .map(|line| {
            match line
                .trim()
                .strip_prefix("#include")
                .map(|rest| rest.trim().trim_matches('"'))
            {
                Some(file) => resolve_includes(&dir.join(file)),
                None => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
