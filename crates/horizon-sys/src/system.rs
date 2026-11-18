//! Mesures système : processeur, mémoire (sysinfo), GPU NVIDIA (NVML), processus gourmands.

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use nvml_wrapper::error::NvmlError;
use serde::Serialize;
use std::time::{Duration, Instant};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, System};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TopProcess {
    pub name: String,
    /// Pourcentage du processeur entier (0–100).
    pub cpu: f32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SystemSample {
    pub cpu: f32,
    pub gpu: Option<f32>,
    pub ram_used: u64,
    pub ram_total: u64,
    pub gpu_temp: Option<f32>,
    /// Rempli seulement si demandé (panneau ouvert).
    pub top: Vec<TopProcess>,
}

/// Délai avant de rouvrir NVML après une perte de la carte (pilote redémarré par Windows,
/// « TDR ») : le pilote se recharge en plusieurs étapes, et lire le GPU avec un accès ouvert
/// avant le redémarrage a fait planter Horizon dans nvml.dll (violation d'accès, impossible à
/// rattraper).
const GPU_REOPEN_AFTER: Duration = Duration::from_secs(30);

pub struct Sampler {
    sys: System,
    nvml: Option<Nvml>,
    /// NVML fermé après une perte de la carte : moment où le rouvrir. `None` avec `nvml` vide :
    /// pas de GPU NVIDIA, on ne réessaie pas.
    reopen_at: Option<Instant>,
    cores: f32,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

/// Erreurs qui signalent une carte perdue ou un pilote en cours de redémarrage (et non une
/// mesure simplement absente sur ce modèle).
fn gpu_lost(e: &NvmlError) -> bool {
    matches!(
        e,
        NvmlError::GpuLost
            | NvmlError::ResetRequired
            | NvmlError::Uninitialized
            | NvmlError::DriverNotLoaded
            | NvmlError::Timeout
            | NvmlError::Unknown
    )
}

impl Sampler {
    pub fn new() -> Sampler {
        let mut sys = System::new();
        sys.refresh_cpu_specifics(CpuRefreshKind::nothing().with_cpu_usage());
        let nvml = Nvml::init()
            .inspect_err(|e| log::info!("NVML indisponible (pas de GPU NVIDIA ?) : {e}"))
            .ok();
        let cores = sys.cpus().len().max(1) as f32;
        Sampler { sys, nvml, reopen_at: None, cores }
    }

    /// Ferme NVML et le rouvre plus tard (`GPU_REOPEN_AFTER`). À appeler quand le pilote
    /// graphique a pu redémarrer (écrans modifiés) : aucun accès d'avant n'est réutilisé.
    pub fn reset_gpu(&mut self, why: &str) {
        if let Some(nvml) = self.nvml.take() {
            log::warn!("GPU : NVML fermé ({why}), réouverture dans {} s", GPU_REOPEN_AFTER.as_secs());
            drop(nvml);
            self.reopen_at = Some(Instant::now() + GPU_REOPEN_AFTER);
        }
    }

    fn reopen_if_due(&mut self) {
        let Some(at) = self.reopen_at else { return };
        if Instant::now() < at {
            return;
        }
        match Nvml::init() {
            Ok(nvml) => {
                log::info!("GPU : NVML rouvert");
                self.nvml = Some(nvml);
                self.reopen_at = None;
            }
            Err(e) => {
                log::info!("GPU : NVML pas encore disponible ({e}), nouvel essai plus tard");
                self.reopen_at = Some(Instant::now() + GPU_REOPEN_AFTER);
            }
        }
    }

    fn gpu(&mut self) -> (Option<f32>, Option<f32>) {
        self.reopen_if_due();
        let Some(nvml) = &self.nvml else {
            return (None, None);
        };
        let read = nvml.device_by_index(0).and_then(|device| {
            let util = device.utilization_rates().map(|u| u.gpu as f32);
            let temp = device.temperature(TemperatureSensor::Gpu).map(|t| t as f32);
            // Une mesure absente sur ce modèle n'empêche pas l'autre ; une carte perdue, si.
            match (util, temp) {
                (Err(e), _) | (_, Err(e)) if gpu_lost(&e) => Err(e),
                (util, temp) => Ok((util.ok(), temp.ok())),
            }
        });
        match read {
            Ok(r) => r,
            Err(e) => {
                if gpu_lost(&e) {
                    self.reset_gpu(&format!("carte perdue : {e}"));
                }
                (None, None)
            }
        }
    }

    /// Une mesure ; à appeler à intervalle régulier (l'usage processeur est un écart entre deux appels).
    pub fn sample(&mut self, with_top: bool) -> SystemSample {
        self.sys.refresh_cpu_specifics(CpuRefreshKind::nothing().with_cpu_usage());
        self.sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        let top = if with_top {
            self.sys.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing().with_cpu(),
            );
            let mut list: Vec<TopProcess> = self
                .sys
                .processes()
                .values()
                .map(|p| TopProcess {
                    name: p.name().to_string_lossy().trim_end_matches(".exe").to_owned(),
                    cpu: p.cpu_usage() / self.cores,
                })
                .filter(|p| p.cpu > 0.0 && p.name != "System Idle Process")
                .collect();
            list.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
            list.truncate(3);
            list
        } else {
            Vec::new()
        };
        let (gpu, gpu_temp) = self.gpu();
        SystemSample {
            cpu: self.sys.global_cpu_usage(),
            gpu,
            ram_used: self.sys.used_memory(),
            ram_total: self.sys.total_memory(),
            gpu_temp,
            top,
        }
    }
}
