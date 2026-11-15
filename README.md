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
./installer.sh
```

`installer.sh` est aussi conçu pour un **double-clic** dans le gestionnaire
de fichiers : il s'ouvre dans une fenêtre de terminal et procède à
l'installation complète (binaire, icône, lanceur pour le dock). Le lanceur
`chat-local.desktop` apparaît alors dans le menu des applications, prêt à
être épinglé dans le dock.
