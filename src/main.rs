mod mem;
mod resolve;

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

use mem::Process;
use resolve::{
    refresh, resolve_player, Resolved, OFF_FLOATING, OFF_G, OFF_VY, PHY_ALWAYS_REWRITE, PHY_BASE_G,
    PHY_FORCE_VY, PHY_G_SCALE, PHY_TRANSLATE_X, PHY_TRANSLATE_Y,
};

const PROC_NAME: &str = "AliceInCradle.exe";
const UP_STACK: f32 = -0.2;
const UP_SPEED: f32 = -0.09;
const TICK: Duration = Duration::from_millis(16);
const VK_W: i32 = 0x57;

fn key_down(vk: i32) -> bool {
    unsafe { GetAsyncKeyState(vk) as u16 & 0x8000 != 0 }
}

fn wait_enter() {
    let _ = io::stdout().flush();
    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line);
}

fn fail_and_wait(msg: &str) -> ! {
    eprintln!("{msg}");
    eprintln!("按 Enter 退出...");
    wait_enter();
    std::process::exit(1);
}

fn apply_fly(proc: &Process, r: &Resolved, fly: bool) {
    if fly {
        let _ = proc.write_u8(r.player + OFF_FLOATING, 1);
        let _ = proc.write_f32(r.player + OFF_G, 0.0);
        let _ = proc.write_f32(r.player + OFF_VY, UP_SPEED);
        if r.phy != 0 {
            let _ = proc.write_f32(r.phy + PHY_TRANSLATE_Y, UP_STACK);
            let _ = proc.write_f32(r.phy + PHY_TRANSLATE_X, 0.0);
            let _ = proc.write_f32(r.phy + PHY_FORCE_VY, UP_SPEED);
            let _ = proc.write_u8(r.phy + PHY_ALWAYS_REWRITE, 1);
            let _ = proc.write_f32(r.phy + PHY_BASE_G, 0.0);
            let _ = proc.write_f32(r.phy + PHY_G_SCALE, 0.0);
        }
    } else {
        let _ = proc.write_u8(r.player + OFF_FLOATING, 0);
        let _ = proc.write_f32(r.player + OFF_VY, 0.0);
        let _ = proc.write_f32(r.player + OFF_G, 0.6);
        if r.phy != 0 {
            let _ = proc.write_f32(r.phy + PHY_TRANSLATE_Y, 0.0);
            let _ = proc.write_f32(r.phy + PHY_TRANSLATE_X, 0.0);
            let _ = proc.write_f32(r.phy + PHY_FORCE_VY, 0.0);
            let _ = proc.write_u8(r.phy + PHY_ALWAYS_REWRITE, 0);
            let _ = proc.write_f32(r.phy + PHY_BASE_G, 0.6);
            let _ = proc.write_f32(r.phy + PHY_G_SCALE, 1.0);
        }
    }
}

fn main() {
    let proc = match Process::open_by_name(PROC_NAME) {
        Ok(p) => p,
        Err(e) if e.contains("process not found") => fail_and_wait(&format!(
            "未找到进程 {PROC_NAME}\n\
             请先启动游戏并进入可操作场景后重试。"
        )),
        Err(e) if e.contains("OpenProcess") => fail_and_wait(&format!(
            "无法打开进程: {e}\n\
             请尝试以管理员身份运行。"
        )),
        Err(e) => fail_and_wait(&format!("错误: {e}")),
    };

    let mut r = match resolve_player(&proc) {
        Ok(v) => v,
        Err(_) => fail_and_wait(
            "已附加进程，但未解析到角色地址。\n\
             请进入游戏世界（可操控角色）后重试。",
        ),
    };

    if let Some(b) = r.base {
        println!("base   = {b:#x}");
    }
    println!("player = {:#x}", r.player);
    println!("phy    = {:#x}", r.phy);
    println!("x={:.3} y={:.3} walk={:.4} run={:.4}", r.x, r.y, r.walk, r.run);
    println!("开启成功（按住 W 起飞，Ctrl+C 退出）");

    apply_fly(&proc, &r, false);

    let running = Arc::new(AtomicBool::new(true));
    {
        let flag = Arc::clone(&running);
        let _ = ctrlc::set_handler(move || {
            flag.store(false, Ordering::SeqCst);
        });
    }

    let mut ticks: u64 = 0;
    let mut was_flying = false;

    while running.load(Ordering::SeqCst) {
        if ticks > 0 && ticks.is_multiple_of(180) {
            if let Ok(nr) = refresh(&proc, &r) {
                r = nr;
            }
        }

        let flying = key_down(VK_W);
        if flying || was_flying {
            apply_fly(&proc, &r, flying);
        }
        was_flying = flying;
        ticks += 1;
        thread::sleep(TICK);
    }

    apply_fly(&proc, &r, false);
}
