use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender};

const MODELS_URL: &str = "http://localhost:13305/v1/models";
const CHAT_URL: &str = "http://localhost:13305/v1/chat/completions";

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
}

/// Événement renvoyé par les threads HTTP vers le thread UI.
enum ApiEvent {
    ModelsLoaded(Vec<LLMConfig>),
    Reply(Message),
    Error(String),
}

struct ChatApp {
    models: Vec<LLMConfig>,
    messages: VecDeque<Message>,
    input_text: String,
    selected_model: String,
    is_generating: bool,
    error: Option<String>,
    event_tx: Sender<ApiEvent>,
    event_rx: Receiver<ApiEvent>,
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
            error: None,
            event_tx,
            event_rx,
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
            let result = (|| -> Result<Vec<LLMConfig>, anyhow::Error> {
                let response = reqwest::blocking::Client::new().get(MODELS_URL).send()?;
                let response = response.error_for_status()?;
                let models_data: ModelsResponse = response.json()?;
                Ok(models_data
                    .data
                    .into_iter()
                    .map(|m| LLMConfig {
                        id: m.id.clone(),
                        model: m.id,
                        endpoint: CHAT_URL.to_string(),
                    })
                    .collect())
            })();

            match result {
                Ok(models) => {
                    let _ = tx.send(ApiEvent::ModelsLoaded(models));
                }
                Err(e) => {
                    let _ = tx.send(ApiEvent::Error(format!("Chargement des modèles : {e}")));
                }
            }
        });
    }

    /// Récupère les résultats des threads HTTP sans bloquer l'UI.
    fn handle_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                ApiEvent::ModelsLoaded(models) => {
                    if let Some(first) = models.first() {
                        self.selected_model = first.id.clone();
                    }
                    self.models = models;
                    self.is_generating = false;
                }
                ApiEvent::Reply(msg) => {
                    self.messages.push_back(msg);
                    self.is_generating = false;
                }
                ApiEvent::Error(e) => {
                    self.error = Some(e);
                    self.is_generating = false;
                }
            }
        }
    }

    /// Envoie le message (et tout l'historique) au LLM dans un thread dédié.
    fn send_message(&mut self) {
        let text = self.input_text.trim().to_string();
        if text.is_empty() || self.models.is_empty() || self.is_generating {
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
        if self.is_generating {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

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
                // Espace pour que la zone de saisie ne recouvre pas le dernier message.
                ui.add_space(60.0);
            });

            // Zone de saisie, en bas du panneau
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                let send = ui.add_enabled(
                    !self.is_generating && !self.models.is_empty(),
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
