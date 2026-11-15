# chat-local

Interface graphique en Rust (eframe/egui) pour discuter avec les LLM locaux servis par [Lemonade](https://github.com/lemonade-sdk/lemonade) via son API compatible OpenAI (`http://localhost:13305/v1`).

## Fonctionnalités

- Chargement des modèles disponibles sur le serveur Lemonade
- Sélection du modèle via un menu déroulant
- Conversation avec historique complet envoyé au modèle à chaque message
- Requêtes HTTP en arrière-plan : l'interface reste réactive
- Affichage des erreurs (serveur injoignable, réponse invalide) directement dans l'UI
- Envoi du message au clic ou avec la touche `Entrée`

## Prérequis

Un serveur Lemonade lancé en local sur le port 13305 (par défaut).

## Compilation

```sh
cargo build --release
```

## Lancement

```sh
./target/release/chat-local
```

## Utilisation

1. Cliquez sur **Charger les modèles**
2. Choisissez un modèle dans le menu déroulant
3. Tapez un message et cliquez sur **Envoyer** (ou appuyez sur `Entrée`)

## Installation (Arch Linux)

```sh
makepkg -si
```
