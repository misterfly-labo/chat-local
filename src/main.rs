use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

const MODELS_URL: &str = "http://localhost:13305/v1/models";
const CHAT_URL: &str = "http://localhost:13305/v1/chat/completions";
const LOAD_URL: &str = "http://localhost:13305/v1/load";
const UNLOAD_URL: &str = "http://localhost:13305/v1/unload";
const HEALTH_URL: &str = "http://localhost:13305/v1/health";
const STATS_URL: &str = "http://localhost:13305/v1/system-stats";
const SYSTEM_INFO_URL: &str = "http://localhost:13305/v1/system-info";
const STATS_REFRESH: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Choice {
    message: Message,
    index: u32,
    finish_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelInfo {
    id: String,
    object: String,
    created: u64,
    owned_by: String,
    /// Taille des poids en Go, si le serveur la renseigne.
    #[serde(default)]
    size: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LLMConfig {
    id: String,
    model: String,
    endpoint: String,
    size_gb: Option<f64>,
}

/// Ressources de la machine, renvoyées par GET /v1/system-stats.
#[derive(Debug, Clone, Deserialize)]
struct SystemStats {
    cpu_percent: Option<f64>,
    #[allow(dead_code)]
    gpu_percent: Option<f64>,
    memory_gb: Option<f64>,
    npu_percent: Option<f64>,
    #[allow(dead_code)]
    vram_gb: Option<f64>,
}

/// Modèle(s) résident(s), renvoyés par GET /v1/health.
#[derive(Debug, Clone, Deserialize)]
struct HealthInfo {
    all_models_loaded: Vec<LoadedModelInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct LoadedModelInfo {
    model_name: String,
    device: String,
    #[allow(dead_code)]
    recipe: String,
    /// PID du process backend qui pilote le modèle (pour lire sa mémoire).
    #[serde(default)]
    pid: u32,
    #[serde(default)]
    loaded: bool,
    /// Mémoire résidente du process backend, remplie côté client.
    #[serde(skip_deserializing)]
    rss_gb: Option<f64>,
}

/// Événement renvoyé par les threads HTTP vers le thread UI.
enum ApiEvent {
    ModelsLoaded(Vec<LLMConfig>, Option<f64>),
    ModelLoaded(String),
    StatsUpdated(Option<SystemStats>, Option<LoadedModelInfo>),
    Reply(Message),
    Error(String),
}

struct ChatApp {
    models: Vec<LLMConfig>,
    messages: VecDeque<Message>,
    input_text: String,
    selected_model: String,
    is_generating: bool,
    /// Modèle en cours de chargement dans le NPU.
    loading_model: bool,
    /// Dernier modèle pour lequel un chargement a été demandé/terminé.
    requested_model: String,
    loaded_model: String,
    error: Option<String>,
    event_tx: Sender<ApiEvent>,
    event_rx: Receiver<ApiEvent>,
    // --- Suivi des ressources ---
    stats: Option<SystemStats>,
    loaded_model_info: Option<LoadedModelInfo>,
    total_ram_gb: Option<f64>,
    stats_loading: bool,
    purge_in_progress: bool,
    last_stats: Instant,
}

/// Lit la mémoire résidente (VmRSS) du process qui pilote le NPU.
/// /proc/<pid>/status est lisible par tous les utilisateurs locaux.
fn read_rss_gb(pid: u32) -> Option<f64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0 / 1024.0)
}

impl ChatApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (event_tx, event_rx) = channel();
        ChatApp {
            models: Vec::new(),
            messages: VecDeque::new(),
            input_text: String::new(),
            selected_model: String::new(),
            is_generating: false,
            loading_model: false,
            requested_model: String::new(),
            loaded_model: String::new(),
            error: None,
            event_tx,
            event_rx,
            stats: None,
            loaded_model_info: None,
            total_ram_gb: None,
            stats_loading: false,
            purge_in_progress: false,
            last_stats: Instant::now() - STATS_REFRESH,
        }
    }

    /// Lance le chargement des modèles dans un thread dédié (l'UI reste réactive).
    fn load_models(&mut self) {
        if self.is_generating {
            return;
        }
        self.is_generating = true;
        self.error = None;

        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<(Vec<LLMConfig>, Option<f64>), anyhow::Error> {
                let client = reqwest::blocking::Client::new();
                let response = client.get(MODELS_URL).send()?;
                let response = response.error_for_status()?;
                let models_data: ModelsResponse = response.json()?;
                let models = models_data
                    .data
                    .into_iter()
                    .map(|m| LLMConfig {
                        id: m.id.clone(),
                        model: m.id,
                        endpoint: CHAT_URL.to_string(),
                        size_gb: m.size,
                    })
                    .collect();

                // RAM physique totale, pour afficher la mémoire en contexte.
                let total_ram_gb = client
                    .get(SYSTEM_INFO_URL)
                    .send()
                    .ok()
                    .and_then(|r| r.error_for_status().ok())
                    .and_then(|r| r.json::<serde_json::Value>().ok())
                    .and_then(|v| {
                        v.get("Physical Memory")
                            .and_then(|s| s.as_str())
                            .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
                    });

                Ok((models, total_ram_gb))
            })();

            match result {
                Ok((models, total_ram_gb)) => {
                    let _ = tx.send(ApiEvent::ModelsLoaded(models, total_ram_gb));
                }
                Err(e) => {
                    let _ = tx.send(ApiEvent::Error(format!("Chargement des modèles : {e}")));
                }
            }
        });
    }

    /// Interroge le serveur (stats système + modèles résidents) dans un thread dédié.
    fn refresh_stats(&mut self) {
        if self.stats_loading {
            return;
        }
        self.stats_loading = true;
        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            let client = reqwest::blocking::Client::new();
            let stats = client
                .get(STATS_URL)
                .send()
                .ok()
                .and_then(|r| r.error_for_status().ok())
                .and_then(|r| r.json::<SystemStats>().ok());
            let loaded = client
                .get(HEALTH_URL)
                .send()
                .ok()
                .and_then(|r| r.error_for_status().ok())
                .and_then(|r| r.json::<HealthInfo>().ok())
                .and_then(|h| {
                    h.all_models_loaded
                        .into_iter()
                        .find(|m| m.loaded)
                })
                .map(|mut m| {
                    m.rss_gb = read_rss_gb(m.pid);
                    m
                });
            let _ = tx.send(ApiEvent::StatsUpdated(stats, loaded));
        });
    }

    /// Décharge le modèle résident du NPU (POST /v1/unload) puis rafraîchit les stats.
    fn purge_npu(&mut self) {
        if self.purge_in_progress {
            return;
        }
        let Some(target) = self
            .loaded_model_info
            .as_ref()
            .map(|m| m.model_name.clone())
            .or_else(|| {
                if self.selected_model.is_empty() {
                    None
                } else {
                    Some(self.selected_model.clone())
                }
            })
        else {
            return;
        };

        self.purge_in_progress = true;
        self.error = None;
        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            let client = reqwest::blocking::Client::new();
            let result = client
                .post(UNLOAD_URL)
                .json(&serde_json::json!({ "model_name": target }))
                .send()
                .and_then(|r| r.error_for_status());

            match result {
                Ok(_) => {
                    // Purge réussie : publier immédiatement l'état vide du NPU.
                    let _ = tx.send(ApiEvent::StatsUpdated(None, None));
                }
                Err(e) => {
                    let _ = tx.send(ApiEvent::Error(format!("Purge du NPU : {e}")));
                }
            }
        });
    }

    /// Taille des poids du modèle résident, d'après la liste des modèles.
    fn loaded_model_size_gb(&self) -> Option<f64> {
        let info = self.loaded_model_info.as_ref()?;
        self.models
            .iter()
            .find(|m| m.id == info.model_name)
            .and_then(|m| m.size_gb)
    }

    /// Charge le modèle sélectionné dans le NPU via POST /v1/load (thread dédié).
    fn load_selected_model(&mut self, force: bool) {
        if self.loading_model || self.selected_model.is_empty() {
            return;
        }
        // Ne recharger que si le modèle demandé change, sauf demande explicite.
        if !force && self.selected_model == self.requested_model {
            return;
        }
        self.requested_model = self.selected_model.clone();
        self.loading_model = true;
        self.error = None;

        let model = self.selected_model.clone();
        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), anyhow::Error> {
                let response = reqwest::blocking::Client::new()
                    .post(LOAD_URL)
                    .json(&serde_json::json!({ "model_name": model }))
                    .send()?;
                let response = response.error_for_status()?;
                let body: serde_json::Value = response.json()?;
                if body.get("status").and_then(|s| s.as_str()) == Some("success") {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("statut inattendu du serveur"))
                }
            })();

            match result {
                Ok(()) => {
                    let _ = tx.send(ApiEvent::ModelLoaded(model));
                }
                Err(e) => {
                    let _ =
                        tx.send(ApiEvent::Error(format!("Chargement du modèle : {e}")));
                }
            }
        });
    }

    /// Récupère les résultats des threads HTTP sans bloquer l'UI.
    fn handle_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                ApiEvent::ModelsLoaded(models, total_ram_gb) => {
                    if let Some(first) = models.first() {
                        self.selected_model = first.id.clone();
                    }
                    self.models = models;
                    self.total_ram_gb = total_ram_gb;
                    self.is_generating = false;
                }
                ApiEvent::ModelLoaded(model) => {
                    self.loaded_model = model;
                    self.loading_model = false;
                }
                ApiEvent::StatsUpdated(stats, loaded) => {
                    self.stats = stats;
                    self.loaded_model = loaded
                        .as_ref()
                        .map(|m| m.model_name.clone())
                        .unwrap_or_default();
                    self.loaded_model_info = loaded;
                    self.stats_loading = false;
                    self.last_stats = Instant::now();
                }
                ApiEvent::Reply(msg) => {
                    self.messages.push_back(msg);
                    self.is_generating = false;
                }
                ApiEvent::Error(e) => {
                    // Éviter une boucle de réessai auto : considérer le modèle comme traité.
                    if self.loading_model {
                        self.loaded_model = self.selected_model.clone();
                    }
                    self.error = Some(e);
                    self.is_generating = false;
                    self.loading_model = false;
                    self.purge_in_progress = false;
                }
            }
        }
    }

    /// Envoie le message (et tout l'historique) au LLM dans un thread dédié.
    fn send_message(&mut self) {
        let text = self.input_text.trim().to_string();
        if text.is_empty() || self.models.is_empty() || self.is_generating || self.loading_model {
            return;
        }
        let Some(config) = self
            .models
            .iter()
            .find(|m| m.id == self.selected_model)
            .cloned()
        else {
            return;
        };

        // Cloner le texte AVANT de vider le champ de saisie.
        self.input_text.clear();
        self.messages
            .push_back(Message { role: "user".to_string(), content: text });

        self.is_generating = true;
        self.error = None;

        let history: Vec<Message> = self.messages.iter().cloned().collect();
        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<Message, anyhow::Error> {
                let response = reqwest::blocking::Client::new()
                    .post(&config.endpoint)
                    .json(&serde_json::json!({
                        "model": config.model,
                        "messages": history,
                    }))
                    .send()?;
                let response = response.error_for_status()?;
                let chat: ChatCompletionResponse = response.json()?;
                chat.choices
                    .into_iter()
                    .next()
                    .map(|c| c.message)
                    .ok_or_else(|| anyhow::anyhow!("réponse vide du serveur"))
            })();

            match result {
                Ok(msg) => {
                    let _ = tx.send(ApiEvent::Reply(msg));
                }
                Err(e) => {
                    let _ = tx.send(ApiEvent::Error(format!("Envoi du message : {e}")));
                }
            }
        });
    }
}

impl eframe::App for ChatApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events();

        // Garder l'UI animée (spinner) tant qu'une requête est en cours.
        if self.is_generating || self.loading_model || self.stats_loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else if self.last_stats.elapsed() >= STATS_REFRESH {
            self.refresh_stats();
        } else {
            // Repainir à l'échéance du prochain rafraîchissement des stats.
            ctx.request_repaint_after(STATS_REFRESH - self.last_stats.elapsed());
        }

        // Barre de saisie dans un panneau fixe en bas : toujours visible et cliquable.
        egui::TopBottomPanel::bottom("input_bar").show(ctx, |ui| {
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                let send = ui.add_enabled(
                    !self.is_generating && !self.loading_model && !self.models.is_empty(),
                    egui::Button::new("Envoyer"),
                );
                let input = ui.add_sized(
                    [ui.available_width() - 10.0, 30.0],
                    egui::TextEdit::singleline(&mut self.input_text).hint_text("Tapez votre message…"),
                );

                // Envoyer au clic ou avec Entrée quand le champ a le focus.
                let enter = input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if send.clicked() || enter {
                    input.request_focus();
                    self.send_message();
                }
            });
            ui.add_space(5.0);
        });

        // Barre de suivi des ressources : RAM, CPU/GPU/NPU, mémoire du modèle, purge.
        egui::TopBottomPanel::bottom("stats_bar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("📊");
                match &self.stats {
                    Some(stats) => {
                        let ram = stats.memory_gb.unwrap_or(0.0);
                        let total = self.total_ram_gb.unwrap_or(0.0);
                        ui.weak(if total > 0.0 {
                            format!("RAM : {ram:.1} / {total:.1} Go")
                        } else {
                            format!("RAM : {ram:.1} Go")
                        });
                        if let Some(cpu) = stats.cpu_percent {
                            ui.separator();
                            ui.weak(format!("CPU : {cpu:.0} %"));
                        }
                        if let Some(npu) = stats.npu_percent {
                            ui.separator();
                            ui.weak(format!("NPU : {npu:.0} %"));
                        }
                    }
                    None => {
                        ui.weak(if self.stats_loading {
                            "Lecture des ressources…"
                        } else {
                            "Ressources indisponibles"
                        });
                    }
                }

                ui.separator();
                match &self.loaded_model_info {
                    Some(m) => {
                        ui.weak(format!("🧠 {} ({})", m.model_name, m.device));
                        if let Some(rss) = m.rss_gb {
                            ui.weak(format!("{rss:.1} Go en mémoire"));
                        } else if let Some(size) = self.loaded_model_size_gb() {
                            ui.weak(format!("≈ {size:.1} Go de poids"));
                        }
                    }
                    None => {
                        ui.weak("🧠 NPU : aucun modèle résident");
                    }
                }

                // Bouton de purge, aligné à droite.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let purge = ui.add_enabled(
                        self.loaded_model_info.is_some() && !self.purge_in_progress,
                        egui::Button::new("🧹 Purger le NPU"),
                    );
                    if purge.clicked() {
                        self.purge_npu();
                    }
                    if self.purge_in_progress {
                        ui.add(egui::Spinner::default());
                    }
                });
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Chat Lemonade");
            ui.add_space(10.0);

            // Barre : sélection du modèle + statut
            ui.horizontal(|ui| {
                ui.label("Modèle :");
                if self.models.is_empty() {
                    if ui.button("Charger les modèles").clicked() {
                        self.load_models();
                    }
                    ui.weak("Aucun modèle chargé");
                } else {
                    egui::ComboBox::from_id_source("model_selector")
                        .selected_text(&self.selected_model)
                        .show_ui(ui, |ui| {
                            for model in &self.models {
                                ui.selectable_value(
                                    &mut self.selected_model,
                                    model.id.clone(),
                                    &model.id,
                                );
                            }
                        });
                    if ui.button("Actualiser").clicked() {
                        self.load_models();
                    }
                    if ui.button("Charger dans le NPU").clicked() {
                        self.load_selected_model(true);
                    }

                    // Charger automatiquement le modèle dès que la sélection change.
                    self.load_selected_model(false);
                }

                // État de chargement du modèle dans le NPU.
                if self.loading_model {
                    ui.add(egui::Spinner::default());
                    ui.weak(format!("Chargement de « {} » dans le NPU…", self.selected_model));
                } else if self.selected_model == self.loaded_model && !self.loaded_model.is_empty() {
                    ui.weak("✅ chargé");
                }
                if self.is_generating {
                    ui.add(egui::Spinner::default());
                }
            });

            // Erreurs affichées dans l'UI, plus uniquement dans le terminal.
            if let Some(error) = &self.error {
                ui.add_space(5.0);
                ui.colored_label(egui::Color32::RED, error);
            }

            ui.add_space(10.0);

            // Historique de la conversation
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                if self.messages.is_empty() {
                    ui.weak(if self.models.is_empty() {
                        "Cliquez sur « Charger les modèles » pour commencer."
                    } else {
                        "Tapez un message pour discuter."
                    });
                }
                for msg in &self.messages {
                    ui.horizontal(|ui| {
                        if msg.role == "user" {
                            ui.label("👤");
                            ui.colored_label(egui::Color32::LIGHT_BLUE, &msg.content);
                        } else {
                            ui.label("🤖");
                            ui.colored_label(egui::Color32::LIGHT_GREEN, &msg.content);
                        }
                    });
                    ui.separator();
                }
            });
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([600.0, 500.0])
            .with_min_inner_size([400.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Chat Lemonade",
        options,
        Box::new(|cc| Ok(Box::new(ChatApp::new(cc)))),
    )
}
