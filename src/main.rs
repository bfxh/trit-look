//! CLI：`look`（全量描述量）/ `style`（六维因子 + Cohen's d）/ `selfcheck`（门自检）。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use trit_look::Look;

fn arg<'a>(argv: &'a [String], key: &str) -> Option<&'a str> {
    argv.iter()
        .position(|a| a == key)
        .and_then(|i| argv.get(i + 1))
        .map(|s| s.as_str())
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}

/// 读 PNG，返回 `(w, h, RGB f32)`；索引色会报错（请先转真彩）。
fn read_png(path: &Path) -> Result<(usize, usize, Vec<f32>), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => return Err("索引色 PNG 未展开".into()),
    };
    let mut out = Vec::with_capacity(w * h * 3);
    for p in 0..w * h {
        let base = p * channels;
        let (r, g, b) = match channels {
            1 | 2 => (buf[base], buf[base], buf[base]),
            _ => (buf[base], buf[base + 1], buf[base + 2]),
        };
        out.push(f32::from(r) / 255.0);
        out.push(f32::from(g) / 255.0);
        out.push(f32::from(b) / 255.0);
    }
    Ok((w, h, out))
}

fn write_png(path: &Path, w: usize, h: usize, rgb: &[f32]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut wtr = enc.write_header().map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = rgb
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8)
        .collect();
    wtr.write_image_data(&bytes).map_err(|e| e.to_string())
}

fn expand(inputs: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for a in inputs {
        let p = PathBuf::from(a);
        if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
                .unwrap_or_else(|e| fail(&format!("读不了 {}: {e}", p.display())))
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|q| q.extension().is_some_and(|x| x == "png"))
                .collect();
            v.sort();
            out.extend(v);
        } else {
            out.push(p);
        }
    }
    out
}

fn positional(argv: &[String], stop_at: Option<&str>) -> Vec<String> {
    let mut v = Vec::new();
    let mut skip = false;
    for a in argv.iter().skip(1) {
        if skip {
            skip = false;
            continue;
        }
        if a == "--out" {
            skip = true;
            continue;
        }
        if let Some(k) = stop_at {
            if a == k {
                break;
            }
        }
        if a.starts_with("--") {
            continue;
        }
        v.push(a.clone());
    }
    v
}

fn look_cmd(argv: &[String]) -> ExitCode {
    let files = expand(&positional(argv, None));
    if files.is_empty() {
        fail("用法: trit-look look <图.png 或目录>… [--out features.csv]");
    }
    let mut out = std::io::stdout().lock();
    let _ = writeln!(
        out,
        "{:<24} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>6} {:>6}",
        "样本",
        "亮度",
        "对比",
        "饱和",
        "色相熵",
        "细纹",
        "硬边",
        "平坦",
        "级数",
        "雾感",
        "发光",
        "笔触",
        "hf1",
        "边界"
    );
    let mut rows: Vec<(String, Look)> = Vec::new();
    for f in &files {
        let name = f
            .file_stem()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (w, h, rgb) = read_png(f).unwrap_or_else(|e| fail(&e));
        let l = trit_look::analyze(&rgb, w, h);
        let _ = writeln!(
            out,
            "{name:<24} {:>5.2} {:>5.2} {:>5.2} {:>5.2} {:>5.3} {:>5.3} {:>5.2} {:>5.0} {:>5.2} {:>5.3} {:>5.2} {:>6.3} {:>6.3}",
            l.luma, l.contrast, l.sat, l.hue_entropy, l.fine, l.strong, l.flat, l.levels, l.haze,
            l.emissive, l.stroke, l.hf1, l.border
        );
        rows.push((name, l));
    }
    if let Some(p) = arg(argv, "--out") {
        let mut csv = String::from(
            "name,luma,contrast,sat,hue_entropy,edge,fine,strong,flat,levels,haze,emissive,bars,warmth,key_dir,key_fill_dwarm,highlights,hf1,hf2,hf4,border,stroke\n",
        );
        for (n, l) in &rows {
            csv.push_str(&format!(
                "{n},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.1},{:.4},{:.4},{:.4},{:.4},{:.1},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.2}\n",
                l.luma, l.contrast, l.sat, l.hue_entropy, l.edge, l.fine, l.strong, l.flat, l.levels,
                l.haze, l.emissive, l.bars, l.warmth, l.key_dir, l.key_fill_dwarm, l.highlights,
                l.hf1, l.hf2, l.hf4, l.border, l.stroke
            ));
        }
        std::fs::write(p, csv).unwrap_or_else(|e| fail(&format!("{p}: {e}")));
        let _ = writeln!(out, "特征已写 {p}");
    }
    ExitCode::SUCCESS
}

fn factors(l: &Look) -> [f32; 6] {
    // 六维：笔触 / 级数 / 硬边 / 对比 / 色温 / 饱和
    [l.stroke, l.levels, l.strong, l.contrast, l.warmth, l.sat]
}

fn measure(files: &[PathBuf]) -> Vec<(String, [f32; 6])> {
    files
        .iter()
        .map(|f| {
            let (w, h, rgb) = read_png(f).unwrap_or_else(|e| fail(&e));
            let l = trit_look::analyze(&rgb, w, h);
            let name = f
                .file_stem()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name, factors(&l))
        })
        .collect()
}

fn mean_std(rows: &[(String, [f32; 6])], k: usize) -> (f32, f32) {
    let n = rows.len().max(1) as f32;
    let m = rows.iter().map(|(_, v)| v[k]).sum::<f32>() / n;
    let var = rows
        .iter()
        .map(|(_, v)| (v[k] - m) * (v[k] - m))
        .sum::<f32>()
        / n;
    (m, var.sqrt())
}

fn style_cmd(argv: &[String]) -> ExitCode {
    let a_files = expand(&positional(argv, Some("--vs")));
    let b_files = if argv.iter().any(|a| a == "--vs") {
        let idx = argv.iter().position(|a| a == "--vs").unwrap();
        expand(&positional(&argv[idx..], None))
    } else {
        Vec::new()
    };
    if a_files.is_empty() {
        fail("用法: trit-look style <A 图/目录…> [--vs <B 图/目录…>] [--out csv]");
    }
    let a = measure(&a_files);
    let b = measure(&b_files);
    let names = ["笔触", "级数", "硬边", "对比", "色温", "饱和"];

    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "A 组 {} 张；B 组 {} 张", a.len(), b.len());
    let _ = writeln!(
        out,
        "{:<6} {:>15} {:>15} {:>7}",
        "因子", "A 均值±std", "B 均值±std", "d"
    );
    let mut separable = 0;
    for (k, name) in names.iter().enumerate() {
        let (ma, sa) = mean_std(&a, k);
        if b.is_empty() {
            let _ = writeln!(out, "{name:<6} {ma:>10.3}±{sa:<5.3}");
            continue;
        }
        let (mb, sb) = mean_std(&b, k);
        let pooled = ((sa * sa * a.len() as f32 + sb * sb * b.len() as f32)
            / (a.len() + b.len()).max(1) as f32)
            .sqrt()
            .max(1e-6);
        let d = (ma - mb) / pooled;
        if d.abs() >= 1.0 {
            separable += 1;
        }
        let _ = writeln!(
            out,
            "{name:<6} {ma:>10.3}±{sa:<5.3} {mb:>10.3}±{sb:<5.3} {d:>7.2}"
        );
    }
    if !b.is_empty() {
        let _ = writeln!(out, "可分开的因子：{separable}/6（|d|≥1）");
    }
    if let Some(p) = arg(argv, "--out") {
        let mut csv = String::from("group,name,stroke,levels,strong,contrast,warmth,sat\n");
        for (g, rows) in [("A", &a), ("B", &b)] {
            for (n, v) in rows.iter() {
                csv.push_str(&format!(
                    "{g},{n},{:.3},{:.1},{:.4},{:.4},{:.4},{:.4}\n",
                    v[0], v[1], v[2], v[3], v[4], v[5]
                ));
            }
        }
        std::fs::write(p, csv).unwrap_or_else(|e| fail(&format!("{p}: {e}")));
        let _ = writeln!(out, "因子表已写 {p}");
    }
    ExitCode::SUCCESS
}

/// 门自检：把「风格度量必须成立的不变量」逐条跑一遍，任一条失败就 exit 1。
fn selfcheck() -> ExitCode {
    let mut fails = 0usize;
    let mut check = |name: &str, ok: bool, detail: String| {
        println!("{} {name}  {detail}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            fails += 1;
        }
    };

    let (w, h) = (64usize, 64usize);
    // 1) 常数图
    let flat = vec![0.5f32; w * h * 3];
    let l1 = trit_look::analyze(&flat, w, h);
    check(
        "常数图",
        (l1.luma - 0.5).abs() < 1e-6 && l1.edge < 1e-6 && l1.flat > 0.99 && l1.levels <= 1.0,
        format!(
            "luma={:.3} edge={:.2e} flat={:.3} levels={:.0}",
            l1.luma, l1.edge, l1.flat, l1.levels
        ),
    );

    // 2) 确定性：同输入两次逐位一致
    let mut rnd = vec![0.0f32; w * h * 3];
    let mut s = 0x1234_5678u64;
    for v in rnd.iter_mut() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        *v = (s >> 11) as f32 / (1u64 << 53) as f32;
    }
    let a1 = trit_look::analyze(&rnd, w, h);
    let a2 = trit_look::analyze(&rnd, w, h);
    check("确定性逐位一致", a1 == a2, "两次 analyze 完全相同".into());

    // 3) 细/粗图案必须分开
    let mut fine = vec![0.0f32; w * h * 3];
    let mut coarse = vec![0.0f32; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let f = if (x + y) % 2 == 0 { 0.3 } else { 0.7 };
            let c = if ((x / 16) + (y / 16)) % 2 == 0 {
                0.3
            } else {
                0.7
            };
            let p = (y * w + x) * 3;
            fine[p..p + 3].fill(f);
            coarse[p..p + 3].fill(c);
        }
    }
    let lf = trit_look::analyze(&fine, w, h);
    let lc = trit_look::analyze(&coarse, w, h);
    check(
        "笔触尺度可分",
        lf.stroke < 2.5 && lc.stroke > 4.0 && lf.hf1 > lc.hf1,
        format!(
            "细 {:.2} / 粗 {:.2}；hf1 {:.2} / {:.2}",
            lf.stroke, lc.stroke, lf.hf1, lc.hf1
        ),
    );

    // 4) 平滑失真方向：把细噪声抹平成局部均值，hf1 必须下降
    let mut blurred = fine.clone();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let mut acc = 0.0f32;
            for dy in 0..3 {
                for dx in 0..3 {
                    acc += fine[((y + dy - 1) * w + (x + dx - 1)) * 3];
                }
            }
            blurred[(y * w + x) * 3..(y * w + x) * 3 + 3].fill(acc / 9.0);
        }
    }
    let lb = trit_look::analyze(&blurred, w, h);
    check(
        "平滑后 hf1 下降",
        lb.hf1 < lf.hf1 * 0.8,
        format!("{:.3} -> {:.3}", lf.hf1, lb.hf1),
    );

    // 5) 分组可分性：高对比方块 vs 低对比噪声，对比度 |d| 应 ≥1
    let group = |kind: u8| -> Vec<f32> {
        let mut v = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let c = match kind {
                    0 => {
                        if ((x / 8) + (y / 8)) % 2 == 0 {
                            0.1
                        } else {
                            0.9
                        }
                    }
                    _ => 0.45 + 0.05 * (((x * 7 + y * 13) % 11) as f32 / 10.0 - 0.5),
                };
                let p = (y * w + x) * 3;
                v[p..p + 3].fill(c);
            }
        }
        v
    };
    let ga: Vec<[f32; 6]> = (0..8)
        .map(|i| {
            let mut g = group(0);
            g[i] = g[i].max(0.0);
            factors(&trit_look::analyze(&g, w, h))
        })
        .collect();
    let gb: Vec<[f32; 6]> = (0..8)
        .map(|i| {
            let mut g = group(1);
            g[i] = g[i].max(0.0);
            factors(&trit_look::analyze(&g, w, h))
        })
        .collect();
    let ma = ga.iter().map(|v| v[3]).sum::<f32>() / ga.len() as f32;
    let mb = gb.iter().map(|v| v[3]).sum::<f32>() / gb.len() as f32;
    let d = (ma - mb) / 0.01f32;
    check(
        "组间对比度可分（|d|≥1）",
        d.abs() >= 1.0,
        format!("d={d:.2}（A {ma:.3} / B {mb:.3}）"),
    );

    // 6) PNG 往返一致
    let tmp = std::env::temp_dir().join("trit_look_selfcheck.png");
    // 8bit PNG 会有量化，所以拿「量化后的同一数据」做对照，而不是原始浮点
    let quant: Vec<f32> = fine
        .iter()
        .map(|v| ((v.clamp(0.0, 1.0) * 255.0) as u8) as f32 / 255.0)
        .collect();
    let ok_io = write_png(&tmp, w, h, &fine).is_ok()
        && read_png(&tmp)
            .map(|(rw, rh, rr)| {
                rw == w
                    && rh == h
                    && trit_look::analyze(&rr, rw, rh) == trit_look::analyze(&quant, w, h)
            })
            .unwrap_or(false);
    let _ = std::fs::remove_file(&tmp);
    check("PNG 往返一致", ok_io, "写盘→读回→analyze 相同".into());

    if fails == 0 {
        println!("SELFCHECK_OK 6/6");
        ExitCode::SUCCESS
    } else {
        println!("SELFCHECK_FAIL {fails} 项");
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("look") => look_cmd(&argv),
        Some("style") => style_cmd(&argv),
        Some("selfcheck") => selfcheck(),
        _ => {
            eprintln!(
                "trit-look {}\n用法:\n  trit-look look <图或目录>… [--out features.csv]\n  trit-look style <A…> [--vs <B…>] [--out style.csv]\n  trit-look selfcheck",
                env!("CARGO_PKG_VERSION")
            );
            ExitCode::from(2)
        }
    }
}
