use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
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

// Timeouts adaptés à chaque type de requête : la génération LLM et le
// chargement d'un modèle dans le NPU peuvent prendre plusieurs minutes.
const FAST_TIMEOUT: Duration = Duration::from_secs(10);
const CHAT_TIMEOUT: Duration = Duration::from_secs(600);
const LOAD_TIMEOUT: Duration = Duration::from_secs(900);

fn http_client(timeout: Duration) -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .expect("création du client HTTP")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
    /// Temps total (aller-retour) pour obtenir cette réponse, en secondes.
    #[serde(skip_serializing, skip_deserializing, default)]
    duration_secs: Option<f64>,
    /// Débit de génération moyen, en tokens/s.
    #[serde(skip_serializing, skip_deserializing, default)]
    tokens_per_sec: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Choice {
    message: Message,
    index: u32,
    finish_reason: String,
}

/// Compteurs standard OpenAI renvoyés par Lemonade.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Usage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
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

/// État persisté entre les lancements de l'application.
#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedState {
    last_model: String,
    /// Facteur de zoom de la police (1.0 = taille normale).
    font_scale: Option<f32>,
}

/// Copie dans le presse-papiers ET lance un process détaché qui conserve
/// le contenu après la fermeture de l'application (requis sous Wayland,
/// où le presse-papiers meurt avec l'application qui le possède).
fn copy_persistent(ctx: &egui::Context, text: String) {
    // Copie immédiate, valable pendant que l'app tourne.
    ctx.copy_text(text.clone());
    // Process qui survit à l'app et sert le presse-papiers ensuite.
    let _ = std::process::Command::new("wl-copy")
        .arg(text)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
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
    /// Dernier modèle utilisé, restauré depuis le fichier de config.
    saved_model: String,
    /// Vrai au premier rendu : donner le focus au champ de saisie.
    want_input_focus: bool,
    /// Facteur de zoom de la police, persisté dans la config.
    font_scale: f32,
    scale_applied: bool,
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
        let saved_model = Self::config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<PersistedState>(&s).ok())
            .map(|s| s.last_model)
            .unwrap_or_default();
        let font_scale = Self::config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<PersistedState>(&s).ok())
            .and_then(|s| s.font_scale)
            .unwrap_or(1.0);
        let mut app = ChatApp {
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
            saved_model,
            want_input_focus: true,
            font_scale,
            scale_applied: false,
        };
        // Au démarrage : récupérer la liste des modèles ; le dernier modèle
        // utilisé sera ensuite chargé dans le NPU automatiquement.
        app.load_models();
        app
    }

    /// Chemin du fichier de configuration (XDG ou $HOME/.config).
    fn config_path() -> Option<PathBuf> {
        let dir = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(dir.join("chat-local").join("config.json"))
    }

    /// Enregistre le dernier modèle utilisé et le zoom sur disque.
    fn persist_state(&self) {
        if let Some(path) = Self::config_path() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let state = PersistedState {
                last_model: self.selected_model.clone(),
                font_scale: Some(self.font_scale),
            };
            if let Ok(json) = serde_json::to_string_pretty(&state) {
                let _ = std::fs::write(path, json);
            }
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
                let client = http_client(FAST_TIMEOUT);
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
            let client = http_client(FAST_TIMEOUT);
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
            let client = http_client(LOAD_TIMEOUT);
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

    /// Met en forme la conversation pour copie (texte brut).
    fn conversation_as_text(messages: &VecDeque<Message>) -> String {
        messages
            .iter()
            .map(|m| {
                let who = if m.role == "user" { "👤 Moi" } else { "🤖 Assistant" };
                format!("{who} :\n{}\n", m.content)
            })
            .collect::<Vec<_>>()
            .join("\n")
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
                let response = http_client(LOAD_TIMEOUT)
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
                    // Restaurer le dernier modèle utilisé si possible, sinon le premier.
                    if !models.iter().any(|m| m.id == self.selected_model) {
                        self.selected_model = models
                            .iter()
                            .find(|m| m.id == self.saved_model)
                            .map(|m| m.id.clone())
                            .or_else(|| models.first().map(|m| m.id.clone()))
                            .unwrap_or_default();
                    }
                    self.models = models;
                    self.total_ram_gb = total_ram_gb;
                    self.is_generating = false;
                }
                ApiEvent::ModelLoaded(model) => {
                    self.loaded_model = model;
                    self.loading_model = false;
                    self.saved_model = self.selected_model.clone();
                    self.persist_state();
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
            .push_back(Message {
                role: "user".to_string(),
                content: text,
                duration_secs: None,
                tokens_per_sec: None,
            });
        self.saved_model = self.selected_model.clone();
        self.persist_state();

        self.is_generating = true;
        self.error = None;

        let history: Vec<Message> = self.messages.iter().cloned().collect();
        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<Message, anyhow::Error> {
                let started = Instant::now();
                let response = http_client(CHAT_TIMEOUT)
                    .post(&config.endpoint)
                    .json(&serde_json::json!({
                        "model": config.model,
                        "messages": history,
                    }))
                    .send()?;
                let response = response.error_for_status()?;
                let chat: ChatCompletionResponse = response.json()?;
                let mut msg = chat
                    .choices
                    .into_iter()
                    .next()
                    .map(|c| c.message)
                    .ok_or_else(|| anyhow::anyhow!("réponse vide du serveur"))?;

                // Temps total et débit moyen : compteurs du serveur si
                // disponibles, sinon estimation par la longueur du texte.
                let elapsed = started.elapsed().as_secs_f64();
                let tokens = chat
                    .usage
                    .map(|u| u.completion_tokens as f64)
                    .unwrap_or_else(|| msg.content.len() as f64 / 4.0);
                if elapsed > 0.0 {
                    msg.duration_secs = Some(elapsed);
                    msg.tokens_per_sec = Some(tokens / elapsed);
                }
                Ok(msg)
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

        // Appliquer le zoom de police restauré, une seule fois.
        if !self.scale_applied {
            if self.font_scale != 1.0 {
                ctx.set_zoom_factor(self.font_scale);
            }
            self.scale_applied = true;
        }

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
                    egui::TextEdit::singleline(&mut self.input_text)
                        .hint_text("Tapez votre message…")
                        .id(egui::Id::new("chat_input")),
                );

                // Au démarrage : le chat est prêt, focus sur le champ de saisie.
                if self.want_input_focus {
                    ctx.memory_mut(|mem| mem.request_focus(egui::Id::new("chat_input")));
                    self.want_input_focus = false;
                }

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
                    if self.is_generating {
                        ui.add(egui::Spinner::default());
                        ui.weak("Récupération des modèles…");
                    } else if ui.button("Charger les modèles").clicked() {
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

                // Gestion de l'historique.
                if !self.messages.is_empty() {
                    if ui.button("📋 Copier tout").clicked() {
                        copy_persistent(ctx, Self::conversation_as_text(&self.messages));
                    }
                    if ui.button("🗑 Effacer").clicked() {
                        self.messages.clear();
                    }
                }

                // Taille des caractères, à droite de la barre.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.separator();
                    if ui.button("A−").clicked() {
                        self.font_scale = (self.font_scale - 0.1).max(0.5);
                        ctx.set_zoom_factor(self.font_scale);
                        self.persist_state();
                    }
                    if ui.button("A+").clicked() {
                        self.font_scale = (self.font_scale + 0.1).min(2.5);
                        ctx.set_zoom_factor(self.font_scale);
                        self.persist_state();
                    }
                    ui.weak(format!("{:.0} %", self.font_scale * 100.0));
                });

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

            // Historique de la conversation, qui suit automatiquement le bas
            // à l'arrivée de nouvelles réponses.
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                if self.messages.is_empty() {
                    ui.weak(if self.models.is_empty() {
                        "Cliquez sur « Charger les modèles » pour commencer."
                    } else {
                        "Tapez un message pour discuter."
                    });
                }
                for msg in &self.messages {
                    ui.horizontal(|ui| {
                        let (icon, color, text) = if msg.role == "user" {
                            ("👤", egui::Color32::LIGHT_BLUE, &msg.content)
                        } else {
                            ("🤖", egui::Color32::LIGHT_GREEN, &msg.content)
                        };
                        ui.label(icon);
                        ui.colored_label(color, text);

                        // Copier ce message en particulier.
                        if ui.small_button("📋").clicked() {
                            copy_persistent(ctx, msg.content.clone());
                        }
                    });
                    // Temps total et débit moyen de la génération.
                    if msg.role != "user" {
                        if let (Some(d), Some(t)) = (msg.duration_secs, msg.tokens_per_sec) {
                            ui.weak(format!("⏱ {d:.1} s · {t:.0} tokens/s"));
                        }
                    }
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
