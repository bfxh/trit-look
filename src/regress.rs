//! S6.2：参考图 → 六维风格因子的**线性回归**（确定性 SGD，权重可存盘）。
//!
//! 口径：因子本身由 `analyze` 定义；这里只学「从像素到因子」的映射。
//! 目的不是取代测量，而是**给没法跑全量分析的场景（视频流、批量筛选）一个便宜的近似**；
//! 任何时候都以 `analyze` 的真值为准，回归只是代理。

use crate::Look;

pub const NF: usize = 6;
/// 特征维度：16×16 灰度缩略图 + 12 桶直方图 + 3 通道均值 + 4 个梯度统计
/// （平均 |Sobel| / 强边占比 / 拉普拉斯能量比 / 结构张量各向异性）。
pub const NFEAT: usize = 16 * 16 + 12 + 3 + 4;

/// 六维因子（顺序固定）：笔触 / 级数 / 硬边 / 对比 / 色温 / 饱和。
pub fn factors(l: &Look) -> [f32; NF] {
    [l.stroke, l.levels, l.strong, l.contrast, l.warmth, l.sat]
}

pub fn features(rgb: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut f = Vec::with_capacity(NFEAT);
    // 16×16 盒式缩略图（灰度）
    let gray = |p: usize| 0.299 * rgb[p * 3] + 0.587 * rgb[p * 3 + 1] + 0.114 * rgb[p * 3 + 2];
    for by in 0..16 {
        for bx in 0..16 {
            let (y0, y1) = (by * h / 16, ((by + 1) * h / 16).max(by * h / 16 + 1));
            let (x0, x1) = (bx * w / 16, ((bx + 1) * w / 16).max(bx * w / 16 + 1));
            let (mut s, mut n) = (0.0f32, 0usize);
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    s += gray(y * w + x);
                    n += 1;
                }
            }
            f.push(s / n.max(1) as f32);
        }
    }
    // 12 桶亮度直方图（归一化）
    let mut hist = [0.0f32; 12];
    let n = w * h;
    for p in 0..n {
        let v = gray(p).clamp(0.0, 1.0);
        hist[((v * 11.0) as usize).min(11)] += 1.0;
    }
    for v in hist.iter() {
        f.push(v / n.max(1) as f32);
    }
    // 通道均值
    let (mut r, mut g, mut b) = (0.0f32, 0.0f32, 0.0f32);
    for p in 0..n {
        r += rgb[p * 3];
        g += rgb[p * 3 + 1];
        b += rgb[p * 3 + 2];
    }
    f.push(r / n.max(1) as f32);
    f.push(g / n.max(1) as f32);
    f.push(b / n.max(1) as f32);
    // 梯度类特征：「硬边」靠缩略图和直方图看不见（实测 r=0.11），必须显式给
    let lum: Vec<f32> = (0..n).map(gray).collect();
    let (mut gm, mut strong_frac, mut lap, mut lap_n) = (0.0f32, 0.0f32, 0.0f32, 0usize);
    let (mut jxx, mut jyy, mut jxy, mut jn) = (0.0f32, 0.0f32, 0.0f32, 0usize);
    let (mut cnt, mut lmean) = (0usize, 0.0f32);
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let at = |dx: isize, dy: isize| {
                lum[((y as isize + dy) as usize) * w + (x as isize + dx) as usize]
            };
            let gx =
                -at(-1, -1) - 2.0 * at(-1, 0) - at(-1, 1) + at(1, -1) + 2.0 * at(1, 0) + at(1, 1);
            let gy =
                -at(-1, -1) - 2.0 * at(0, -1) - at(1, -1) + at(-1, 1) + 2.0 * at(0, 1) + at(1, 1);
            let m = (gx * gx + gy * gy).sqrt() / 4.0;
            gm += m;
            if m > 0.3 {
                strong_frac += 1.0;
            }
            let c = lum[y * w + x];
            lap += (4.0 * c - at(-1, 0) - at(1, 0) - at(0, -1) - at(0, 1)).abs();
            lmean += c;
            lap_n += 1;
            if m > 0.05 {
                jxx += gx * gx;
                jyy += gy * gy;
                jxy += gx * gy;
                jn += 1;
            }
            cnt += 1;
        }
    }
    let cntf = cnt.max(1) as f32;
    f.push(gm / cntf);
    f.push(strong_frac / cntf);
    f.push(lap / lap_n.max(1) as f32 / (lmean / lap_n.max(1) as f32).max(1e-3));
    let coh = if jn > 0 && (jxx + jyy) > 1e-9 {
        let tr = jxx + jyy;
        let det = jxx * jyy - jxy * jxy;
        let disc = (tr * tr - 4.0 * det).max(0.0).sqrt();
        ((disc) / tr).clamp(0.0, 1.0)
    } else {
        0.0
    };
    f.push(coh);
    f
}

#[derive(Debug, Clone)]
pub struct LinModel {
    pub fmean: Vec<f32>,
    pub fstd: Vec<f32>,
    /// `w[t][k]`：第 t 个因子在第 k 个特征上的权重。
    pub w: Vec<[f32; NF]>,
    pub tmean: [f32; NF],
    pub tstd: [f32; NF],
}

impl LinModel {
    /// 确定性最小二乘（标准化特征 + 全量梯度下降）。同样的样本顺序 ⇒ 同样的权重。
    pub fn fit(samples: &[(Vec<f32>, [f32; NF])]) -> Self {
        let n = samples.len().max(1);
        let mut fmean = vec![0.0f32; NFEAT];
        for (f, _) in samples {
            for k in 0..NFEAT {
                fmean[k] += f[k];
            }
        }
        for v in fmean.iter_mut() {
            *v /= n as f32;
        }
        let mut fstd = vec![0.0f32; NFEAT];
        for (f, _) in samples {
            for k in 0..NFEAT {
                let d = f[k] - fmean[k];
                fstd[k] += d * d;
            }
        }
        for v in fstd.iter_mut() {
            *v = (*v / n as f32).sqrt().max(1e-4);
        }
        let mut tmean = [0.0f32; NF];
        for (_, t) in samples {
            for (k, v) in t.iter().enumerate() {
                tmean[k] += v;
            }
        }
        for v in tmean.iter_mut() {
            *v /= n as f32;
        }
        let mut tstd = [1.0f32; NF];
        for (_, t) in samples {
            for (k, v) in t.iter().enumerate() {
                let d = v - tmean[k];
                tstd[k] += d * d;
            }
        }
        for v in tstd.iter_mut() {
            *v = ((*v - 1.0).max(0.0) / n as f32).sqrt().max(1e-3);
        }

        // 闭式岭回归（标准化特征）：A = XᵀX + λI，W = A⁻¹XᵀY；Cholesky 求解，确定性且不发散。
        let d = NFEAT;
        let m = NF;
        let nf = samples.len().max(1) as f64;
        let mut a = vec![vec![0.0f64; d]; d];
        let mut b = vec![vec![0.0f64; m]; d];
        for (f, t) in samples {
            let xs: Vec<f64> = (0..d)
                .map(|k| ((f[k] - fmean[k]) / fstd[k]) as f64)
                .collect();
            let ys: Vec<f64> = (0..m)
                .map(|k| ((t[k] - tmean[k]) / tstd[k]) as f64)
                .collect();
            for i in 0..d {
                for j in 0..d {
                    a[i][j] += xs[i] * xs[j];
                }
                for k in 0..m {
                    b[i][k] += xs[i] * ys[k];
                }
            }
        }
        let lam = 1e-2 * nf;
        for (i, row) in a.iter_mut().enumerate() {
            row[i] += lam;
        }
        // Cholesky：A = L·Lᵀ
        let mut l = vec![vec![0.0f64; d]; d];
        for i in 0..d {
            for j in 0..=i {
                let mut acc = a[i][j];
                for (lk_i, lk_j) in l[i][..j].iter().zip(&l[j][..j]) {
                    acc -= lk_i * lk_j;
                }
                l[i][j] = if i == j {
                    acc.max(1e-12).sqrt()
                } else {
                    acc / l[j][j]
                };
            }
        }
        let solve = |rhs: &[f64]| -> Vec<f64> {
            let mut y = vec![0.0f64; d];
            for i in 0..d {
                let mut acc = rhs[i];
                for k in 0..i {
                    acc -= l[i][k] * y[k];
                }
                y[i] = acc / l[i][i];
            }
            let mut x = vec![0.0f64; d];
            for i in (0..d).rev() {
                let mut acc = y[i];
                for k in i + 1..d {
                    acc -= l[k][i] * x[k];
                }
                x[i] = acc / l[i][i];
            }
            x
        };
        let mut w = vec![[0.0f32; NF]; NFEAT];
        for k in 0..m {
            let rhs: Vec<f64> = (0..d).map(|i| b[i][k]).collect();
            let sol = solve(&rhs);
            for j in 0..d {
                w[j][k] = sol[j] as f32;
            }
        }
        LinModel {
            fmean,
            fstd,
            w,
            tmean,
            tstd,
        }
    }

    pub fn predict_feat(&self, f: &[f32]) -> [f32; NF] {
        let xs: Vec<f32> = (0..NFEAT)
            .map(|k| (f[k] - self.fmean[k]) / self.fstd[k])
            .collect();
        let ys: [f32; NF] =
            std::array::from_fn(|k| xs.iter().enumerate().map(|(j, x)| self.w[j][k] * x).sum());
        std::array::from_fn(|k| ys[k] * self.tstd[k] + self.tmean[k])
    }

    pub fn predict(&self, rgb: &[f32], w: usize, h: usize) -> [f32; NF] {
        self.predict_feat(&features(rgb, w, h))
    }

    /// 文本存盘（`LKMD1`）：便于 diff 与人工检查。
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(s, "LKMD1 {} {}", NFEAT, NF);
        for v in [&self.fmean, &self.fstd] {
            for x in v.iter() {
                let _ = write!(s, "{x} ");
            }
            let _ = writeln!(s);
        }
        for row in &self.w {
            for x in row.iter() {
                let _ = write!(s, "{x} ");
            }
            let _ = writeln!(s);
        }
        for v in [&self.tmean, &self.tstd] {
            for x in v.iter() {
                let _ = write!(s, "{x} ");
            }
            let _ = writeln!(s);
        }
        std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let txt = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut it = txt.split_whitespace();
        if it.next() != Some("LKMD1") {
            return Err(format!("{}: 不是 LKMD1", path.display()));
        }
        let nfeat: usize = it.next().and_then(|v| v.parse().ok()).ok_or("缺 nfeat")?;
        let nf: usize = it.next().and_then(|v| v.parse().ok()).ok_or("缺 nf")?;
        if nfeat != NFEAT || nf != NF {
            return Err(format!("维度不符：{nfeat}/{nf}"));
        }
        let mut take = || -> Result<f32, String> {
            it.next()
                .ok_or_else(|| "权重不足".to_string())?
                .parse::<f32>()
                .map_err(|e| e.to_string())
        };
        let mut fmean = vec![0.0; NFEAT];
        let mut fstd = vec![0.0; NFEAT];
        for v in fmean.iter_mut() {
            *v = take()?;
        }
        for v in fstd.iter_mut() {
            *v = take()?;
        }
        let mut w = vec![[0.0f32; NF]; NFEAT];
        for row in w.iter_mut() {
            for v in row.iter_mut() {
                *v = take()?;
            }
        }
        let mut tmean = [0.0f32; NF];
        let mut tstd = [0.0f32; NF];
        for v in tmean.iter_mut() {
            *v = take()?;
        }
        for v in tstd.iter_mut() {
            *v = take()?;
        }
        Ok(LinModel {
            fmean,
            fstd,
            w,
            tmean,
            tstd,
        })
    }
}

/// Pearson 相关（用于门的判据）。
pub fn pearson(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len()).max(1) as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut sab, mut sa, mut sb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        let (dx, dy) = (x - ma, y - mb);
        sab += dx * dy;
        sa += dx * dx;
        sb += dy * dy;
    }
    let denom = (sa * sb).sqrt();
    if denom < 1e-9 {
        0.0 // 常数序列没有相关性，返回 0 而不是 NaN
    } else {
        sab / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_are_deterministic_and_sized() {
        let (w, h) = (32usize, 32usize);
        let mut v = vec![0.0f32; w * h * 3];
        for (i, p) in v.iter_mut().enumerate() {
            *p = if i % 3 == 0 { 0.2 } else { 0.8 };
        }
        assert_eq!(features(&v, w, h), features(&v, w, h));
        assert_eq!(features(&v, w, h).len(), NFEAT);
    }

    #[test]
    fn fit_recovers_synthetic_signal() {
        // 目标 = 特征的一个线性组合（+常数），拟合应能恢复
        let (w, h) = (24usize, 24usize);
        let mut samples = Vec::new();
        let mut s = 0x1234u64;
        for _ in 0..40 {
            let mut v = vec![0.0f32; w * h * 3];
            for p in v.iter_mut() {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                *p = (s >> 11) as f32 / (1u64 << 53) as f32;
            }
            let f = features(&v, w, h);
            let t: [f32; NF] =
                std::array::from_fn(|k| f[k % NFEAT] * 0.5 + f[(k * 7) % NFEAT] * 0.25 + k as f32);
            samples.push((f, t));
        }
        let m = LinModel::fit(&samples);
        let mut rs = Vec::new();
        for k in 0..NF {
            let truth: Vec<f32> = samples.iter().map(|(_, t)| t[k]).collect();
            let pred: Vec<f32> = samples.iter().map(|(f, _)| m.predict_feat(f)[k]).collect();
            rs.push(pearson(&truth, &pred));
        }
        for (k, r) in rs.iter().enumerate() {
            assert!(*r > 0.95, "因子 {k} 应被拟合出来，实际 r={r:.3}");
        }
    }

    #[test]
    fn model_roundtrip() {
        let (w, h) = (8usize, 8usize);
        let v = vec![0.3f32; w * h * 3];
        let f = features(&v, w, h);
        let samples: Vec<(Vec<f32>, [f32; NF])> = (0..4)
            .map(|i| (f.clone(), std::array::from_fn(|k| (i + k) as f32)))
            .collect();
        let m = LinModel::fit(&samples);
        let p = std::env::temp_dir().join("lk_model_test.lk");
        m.save(&p).unwrap();
        let back = LinModel::load(&p).unwrap();
        let a = m.predict_feat(&f);
        let b = back.predict_feat(&f);
        for k in 0..NF {
            assert!((a[k] - b[k]).abs() < 1e-5, "存读应一致");
        }
        let _ = std::fs::remove_file(p);
    }
}
