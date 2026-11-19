# Fleuve

Gestionnaire de téléchargements pour macOS, inspiré du fonctionnement d'Internet Download Manager (IDM).
Projet personnel de Gespère BONAZEBI — ingénierie inverse **fonctionnelle** : aucun code ni fichier d'IDM n'est réutilisé.

## Ce que fait Fleuve

- **Accélération multi-connexions** : jusqu'à 32 connexions par fichier, avec la segmentation dynamique d'IDM (chaque connexion libre reprend la moitié du plus grand segment restant).
- **Reprise** : état sauvegardé toutes les 2 secondes ; reprise après pause, coupure réseau ou redémarrage du Mac.
- **Lien avec le navigateur** : extension Chrome (aussi Edge, Brave, Arc) qui intercepte les téléchargements, ajoute « Télécharger avec Fleuve » au clic droit et détecte les vidéos et sons de la page.
- **Capture vidéo HLS** (.m3u8) : meilleure qualité choisie automatiquement, morceaux téléchargés en parallèle puis assemblés.
- **File d'attente, planificateur, limite de débit**, classement automatique par catégorie.

Fleuve ne contourne aucune protection : les flux chiffrés ou sous DRM sont refusés.

## Prérequis sur le Mac

```bash
xcode-select --install                                   # outils de compilation Apple
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust
brew install node                                        # Node.js 20 ou plus
```

## Lancer et construire

```bash
cd fleuve
npm install
npm run tauri dev      # mode développement, rechargement à chaud
npm run tauri build    # application finale
```

L'application est créée dans `target/release/bundle/macos/Fleuve.app` et l'installeur dans `target/release/bundle/dmg/`.
Glissez `Fleuve.app` dans **Applications**. Comme l'app n'est pas signée par Apple, au premier lancement faites clic droit → **Ouvrir**.

Pour un binaire universel (Apple Silicon + Intel) :

```bash
rustup target add x86_64-apple-darwin aarch64-apple-darwin
npm run tauri build -- --target universal-apple-darwin
```

## Installer l'extension (Chrome, Edge, Brave, Arc)

1. Ouvrez `chrome://extensions` et activez le **Mode développeur**.
2. Cliquez sur **Charger l'extension non empaquetée** et choisissez le dossier `extension/`.
3. Dans Fleuve : **Réglages → Jeton de l'extension → Copier**.
4. Dans les options de l'extension : collez le jeton, puis **Tester la connexion**.

Le lien passe par `127.0.0.1:47821`, uniquement en local, protégé par le jeton.
Si Fleuve est fermé, le navigateur télécharge normalement.

## Tests

```bash
cargo test -p fleuve-core          # 16 tests : segmentation, reprise, HLS, file, limite, sécurité du lien
cargo run -p fleuve-core --example sans_interface -- ./donnees ./telechargements
```

## Organisation du code

| Dossier | Rôle |
| --- | --- |
| `crates/fleuve-core/` | Moteur en Rust : `engine.rs` (segments), `hls.rs`, `manager.rs` (file, planificateur, sauvegarde), `limiter.rs`, `server.rs` (lien navigateur) |
| `src-tauri/` | Application Tauri : commandes appelées par l'interface |
| `src/` | Interface React + TypeScript |
| `extension/` | Extension navigateur (Manifest V3) |

Données de l'app : `~/Library/Application Support/com.gesperebonazebi.fleuve/` (`downloads.json`, `settings.json`).
