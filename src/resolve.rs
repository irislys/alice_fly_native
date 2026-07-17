use crate::mem::Process;

// 定位：Mono 字段偏移（m2d.M2Mover / M2MoverPr / M2Phys），CE mono_class_enumFields 核对
// player+0x38 Phy* | +0x7C floating | +0x88 x_ | +0x8C y_ | +0x94 vy_ | +0xB0 gravity
// player+0x12C walkSpeed(~0.085) | +0x130 runSpeed(~0.17)
// phy+0x178 translate_stack_y（实测主位移）| +0xA8 force_vy | +0xD4 base_g | +0xEC always_rewrite | +0x140 g_scale
// base+0x308 = PlayerNoel（m2d.M2DBase.Instance 链）
pub const OFF_PHY: u64 = 0x38;
pub const OFF_FLOATING: u64 = 0x7C;
pub const OFF_X: u64 = 0x88;
pub const OFF_Y: u64 = 0x8C;
pub const OFF_VY: u64 = 0x94;
pub const OFF_G: u64 = 0xB0;
pub const OFF_WALK: u64 = 0x12C;
pub const OFF_RUN: u64 = 0x130;
pub const PHY_FORCE_VY: u64 = 0xA8;
pub const PHY_BASE_G: u64 = 0xD4;
pub const PHY_ALWAYS_REWRITE: u64 = 0xEC;
pub const PHY_G_SCALE: u64 = 0x140;
pub const PHY_TRANSLATE_X: u64 = 0x174;
pub const PHY_TRANSLATE_Y: u64 = 0x178;
pub const OFF_BASE_PLAYER: u64 = 0x308;

const CHUNK: usize = 0x1_0000;

#[derive(Clone, Debug)]
pub struct Resolved {
    pub player: u64,
    pub phy: u64,
    pub base: Option<u64>,
    pub x: f32,
    pub y: f32,
    pub walk: f32,
    pub run: f32,
    pub score: i32,
}

fn finite_pos(v: f32) -> bool {
    v.is_finite() && v.abs() < 1.0e6
}

fn f32_at(buf: &[u8], off: usize) -> Option<f32> {
    let b = buf.get(off..off + 4)?;
    Some(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(buf: &[u8], off: usize) -> Option<u64> {
    let b = buf.get(off..off + 8)?;
    Some(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

// 定位：以 walk/run/gravity 默认值 + Phy 指针可读 打分，扫堆找 player 对象
fn score_from_buf(
    proc: &Process,
    _addr: u64,
    buf: &[u8],
) -> Option<(i32, f32, f32, f32, f32, f32, u64)> {
    if buf.len() < 0x140 {
        return None;
    }
    let walk = f32_at(buf, OFF_WALK as usize)?;
    let run = f32_at(buf, OFF_RUN as usize)?;
    let g = f32_at(buf, OFF_G as usize)?;
    let x = f32_at(buf, OFF_X as usize)?;
    let y = f32_at(buf, OFF_Y as usize)?;
    let vy = f32_at(buf, OFF_VY as usize)?;
    let phy = u64_at(buf, OFF_PHY as usize)?;

    if !walk.is_finite() || !run.is_finite() || !g.is_finite() {
        return None;
    }
    if !finite_pos(x) || !finite_pos(y) || !vy.is_finite() {
        return None;
    }
    if !(0.01..8.0).contains(&walk) || !(0.01..16.0).contains(&run) {
        return None;
    }
    if run + 1e-4 < walk * 0.5 {
        return None;
    }
    if !(0.0..3.0).contains(&g) {
        return None;
    }
    if !proc.looks_like_user_ptr(phy) {
        return None;
    }

    let mut score: i32 = 10;

    if (walk - 0.085).abs() < 0.002 {
        score += 40;
    } else if (0.05..2.0).contains(&walk) {
        score += 15;
    }
    if (run - 0.17).abs() < 0.004 {
        score += 40;
    } else if run > walk {
        score += 10;
    }
    if (g - 0.6).abs() < 0.02 {
        score += 25;
    } else if g == 0.0 {
        score += 5;
    }

    let sy = proc.read_f32(phy + PHY_TRANSLATE_Y)?;
    if !sy.is_finite() {
        return None;
    }
    score += 15;

    if let Some(bg) = proc.read_f32(phy + PHY_BASE_G)
        && bg.is_finite()
        && (0.0..3.0).contains(&bg)
    {
        score += 10;
        if (bg - 0.6).abs() < 0.05 {
            score += 10;
        }
    }

    if let (Some(sx), Some(szy)) = (f32_at(buf, 0x68), f32_at(buf, 0x6C))
        && sx.is_finite()
        && szy.is_finite()
        && (0.05..5.0).contains(&sx)
        && (0.2..10.0).contains(&szy)
    {
        score += 8;
    }

    Some((score, x, y, walk, run, g, phy))
}

fn score_player(proc: &Process, addr: u64) -> Option<(i32, f32, f32, f32, f32, f32, u64)> {
    let mut buf = [0u8; 0x140];
    if !proc.read_bytes(addr, &mut buf) {
        return None;
    }
    score_from_buf(proc, addr, &buf)
}

fn region_priority(base: u64, size: u64) -> i32 {
    let mut p = 0;
    if size >= 0x1_0000 && size <= 0x100_0000 {
        p += 2;
    }
    if base >= 0x1_0000_0000 && base < 0x8000_0000_0000 {
        p += 1;
    }
    if size < 0x2000 {
        p -= 5;
    }
    p
}

pub fn resolve_player(proc: &Process) -> Result<Resolved, String> {
    let mut regions = proc.readable_regions();
    if regions.is_empty() {
        return Err("no readable regions (permissions?)".into());
    }
    regions.sort_by_key(|&(b, s)| std::cmp::Reverse(region_priority(b, s)));

    let mut best: Option<Resolved> = None;
    let mut scanned: u64 = 0;
    let mut buf = vec![0u8; CHUNK + 0x200];

    for (base, size) in regions {
        if size < 0x200 {
            continue;
        }
        let scan_size = size.min(0x200_0000);
        let mut off: u64 = 0;
        while off < scan_size {
            let want = ((scan_size - off) as usize).min(CHUNK + 0x200);
            let addr = base + off;
            if !proc.read_bytes(addr, &mut buf[..want]) {
                off = off.saturating_add(CHUNK as u64);
                continue;
            }
            let limit = want.saturating_sub(0x140);
            let mut i = 0usize;
            while i < limit {
                scanned += 1;
                let slice = &buf[i..];
                let cand_addr = addr + i as u64;
                if let (Some(walk), Some(run)) =
                    (f32_at(slice, OFF_WALK as usize), f32_at(slice, OFF_RUN as usize))
                    && walk.is_finite()
                    && run.is_finite()
                    && (0.01..8.0).contains(&walk)
                    && (0.01..16.0).contains(&run)
                    && let Some((score, x, y, walk, run, _g, phy)) =
                        score_from_buf(proc, cand_addr, slice)
                    && score >= 50
                {
                    let cand = Resolved {
                        player: cand_addr,
                        phy,
                        base: None,
                        x,
                        y,
                        walk,
                        run,
                        score,
                    };
                    if best.as_ref().map(|b| b.score).unwrap_or(0) < score {
                        best = Some(cand);
                        if score >= 130 {
                            let mut r = best.unwrap();
                            r.base = find_base_for_player(proc, r.player);
                            return Ok(r);
                        }
                    }
                }
                i += 0x10;
            }
            off = off.saturating_add(CHUNK as u64);
        }
    }

    let mut r = best.ok_or_else(|| {
        format!("player not found (scanned ~{scanned} slots). Enter in-world and retry.")
    })?;
    r.base = find_base_for_player(proc, r.player);
    Ok(r)
}

// 定位：在 player 前 0x4000 内找 [addr+0x308]==player 的 M2DBase
fn find_base_for_player(proc: &Process, player: u64) -> Option<u64> {
    const WINDOW: u64 = 0x4000;
    let start = player.saturating_sub(WINDOW) & !0xF;
    let mut a = start;
    while a < player {
        if proc.read_u64(a + OFF_BASE_PLAYER) == Some(player) && proc.looks_like_user_ptr(a) {
            return Some(a);
        }
        a += 0x10;
    }
    None
}

pub fn refresh(proc: &Process, prev: &Resolved) -> Result<Resolved, String> {
    if let Some((score, x, y, walk, run, _g, phy)) = score_player(proc, prev.player)
        && score >= 40
        && phy != 0
    {
        return Ok(Resolved {
            player: prev.player,
            phy,
            base: prev.base.or_else(|| find_base_for_player(proc, prev.player)),
            x,
            y,
            walk,
            run,
            score,
        });
    }
    resolve_player(proc)
}
