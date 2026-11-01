//! 不变量测试：这些是「门」的判据，破坏任何一条都应该红。

use trit_look::{analyze, Look};

fn gray(v: f32, w: usize, h: usize) -> Vec<f32> {
    vec![v; w * h * 3]
}

fn pattern(w: usize, h: usize, block: usize, lo: f32, hi: f32) -> Vec<f32> {
    let mut v = vec![0.0f32; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let c = if ((x / block) + (y / block)) % 2 == 0 {
                lo
            } else {
                hi
            };
            let p = (y * w + x) * 3;
            v[p..p + 3].fill(c);
        }
    }
    v
}

#[test]
fn determinism_bitwise() {
    let w = 48;
    let h = 48;
    let mut v = vec![0.0f32; w * h * 3];
    let mut s = 0xDEAD_BEEFu64;
    for x in v.iter_mut() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        *x = (s >> 11) as f32 / (1u64 << 53) as f32;
    }
    assert_eq!(analyze(&v, w, h), analyze(&v, w, h), "同一输入必须逐位一致");
}

#[test]
fn constant_image_known_values() {
    let l: Look = analyze(&gray(0.5, 32, 32), 32, 32);
    assert!((l.luma - 0.5).abs() < 1e-6);
    assert!(l.edge < 1e-6);
    assert!(l.flat > 0.99);
    assert!(l.levels <= 1.0);
    assert!(l.stroke <= 1.0 + 1e-6, "常数图自相关处处为 1，尺度应取最小");
}

#[test]
fn stroke_and_hf_separate_fine_from_coarse() {
    let (w, h) = (64, 64);
    let fine = pattern(w, h, 1, 0.3, 0.7);
    let coarse = pattern(w, h, 16, 0.3, 0.7);
    let lf = analyze(&fine, w, h);
    let lc = analyze(&coarse, w, h);
    assert!(
        lf.stroke < lc.stroke,
        "细图案尺度应更小：{} vs {}",
        lf.stroke,
        lc.stroke
    );
    assert!(
        lf.hf1 > lc.hf1,
        "细图案高频能量应更大：{} vs {}",
        lf.hf1,
        lc.hf1
    );
}

#[test]
fn contrast_matches_pattern() {
    let (w, h) = (64, 64);
    let hi = analyze(&pattern(w, h, 8, 0.05, 0.95), w, h);
    let lo = analyze(&pattern(w, h, 8, 0.45, 0.55), w, h);
    assert!(
        hi.contrast > lo.contrast * 5.0,
        "高对比图案的 contrast 应显著更大"
    );
}

#[test]
fn border_ratio_flags_padding() {
    // 周期平铺：边界比应≈1
    let (w, h) = (64, 64);
    let periodic = pattern(w, h, 4, 0.2, 0.8);
    let lp = analyze(&periodic, w, h);
    assert!(
        (lp.border - 1.0).abs() < 0.25,
        "周期图边界比应≈1，实际 {}",
        lp.border
    );
}

#[test]
fn selfcheck_gate_passes() {
    // 直接调用二进制的 selfcheck，确保「门」本身是绿的
    let exe = env!("CARGO_BIN_EXE_trit-look");
    let out = std::process::Command::new(exe)
        .arg("selfcheck")
        .output()
        .expect("跑 selfcheck");
    assert!(
        out.status.success(),
        "selfcheck 必须通过：\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
