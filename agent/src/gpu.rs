use crate::types::{GpuDevice, GpuInfo};
use std::process::Command;


pub fn detect_nvidia_gpu() -> Option<GpuInfo>{
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total,memory.used,memory.free,utilization.gpu,temperature.gpu,power.draw,power.limit,driver_version", 
            "--format=csv,noheader,nounits"])
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    
    let mut gpus: Vec<GpuDevice> = Vec::new();
    let mut driver_version: Option<String> = None;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // name, mem_total, mem_used, mem_free, util, temp, pwr_draw, pwr_limit, driver
        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if parts.len() < 9 {
            continue;
        }

        let name = parts[0].to_string();
        let mem_total = parts[1].parse::<u64>().unwrap_or(0);
        let mem_used = parts[2].parse::<u64>().unwrap_or(0);
        let mem_free = parts[3].parse::<u64>().unwrap_or(0);
        let util = parts[4].parse::<u32>().unwrap_or(0);

        // Optional numeric fields can sometimes be "N/A"
        let temp = parts[5].parse::<u32>().ok();
        let pwr_draw = parts[6].parse::<f32>().ok();
        let pwr_limit = parts[7].parse::<f32>().ok();

        if driver_version.is_none() && !parts[8].is_empty() {
            driver_version = Some(parts[8].to_string());
        }

        gpus.push(GpuDevice {
            name,
            memory_total_mib: mem_total,
            memory_used_mib: mem_used,
            memory_free_mib: mem_free,
            utilization_gpu_pct: util,
            temperature_c: temp,
            power_draw_w: pwr_draw,
            power_limit_w: pwr_limit,
        });
    }

    if gpus.is_empty() {
        return None;
    }

    let cuda_version = Command::new("nvidia-smi")
        .output()
        .ok()
        .and_then(|o| {
            if !o.status.success() {
                return None;
            }

            let s = String::from_utf8_lossy(&o.stdout);

            s.lines()
                .find(|l| l.contains("CUDA Version"))
                .and_then(|l| l.split("CUDA Version:").nth(1))
                .map(|v| v.trim().split_whitespace().next().unwrap_or("").to_string())
                .filter(|v| !v.is_empty())
        });


        return Some(GpuInfo { 
            kind: "nvidia".to_string(), 
            count: gpus.len() as u32,  
            driver_version, 
            cuda_version,
            gpus,
        });
}
