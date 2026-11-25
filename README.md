# raaaaah-weeee

Démo pédagogique **Win32 en Rust** pour Windows : une petite fenêtre
`raaaaah weeee` s'ouvre, et chaque fois qu'on la ferme, cinq autres
apparaissent à des positions aléatoires.

L'intérêt est technique : `CreateWindowExW` + `WM_PAINT` + `WM_DESTROY`,
une boucle de messages par fenêtre, et l'API MCI (`mciSendStringW`) pour
jouer un mp3 à chaque ouverture. Pas de `MessageBoxW` (impossible à placer).

## Comportement

- **1 fenêtre au lancement**, centrée sur l'écran principal.
- **À la fermeture : 5 fenêtres** (croix ou bouton OK, un seul chemin via
  `WM_DESTROY`), positions aléatoires sur le bureau virtuel, sans
  recouvrement entre elles.
- **Profondeur par défaut : 3 générations**, soit **31 fenêtres** au total
  (`(5^n − 1) / 4`). Le programme rend la main quand la dernière est fermée.
- `--depth n` (ou `n` seul) : générations `0` à `n−1` — 3 → 31, 4 → 156, 5 → 781.
- `--infinite` : retire le plafond et ouvre 2 fenêtres de plus par seconde,
  indéfiniment. À réserver à une VM jetable.
- `--sound <fichier>` : mp3 joué à chaque ouverture. À défaut, le programme
  cherche `AAAAAHWEEEE.mp3`, `raaaaah.mp3`, `sound.mp3` ou `popup.mp3` à côté
  de l'exe puis dans le répertoire courant. Sans mp3 : muet.

## Compilation

```sh
cargo build --release --target x86_64-pc-windows-gnu
```

La cible GNU fonctionne aussi depuis Linux/WSL (paquet `mingw-w64`). Depuis
Windows en MSVC :

```powershell
cargo build --release --target x86_64-pc-windows-msvc
```

## Utilisation

```powershell
.\raaaaah-weeee.exe                # 31 fenêtres max (défaut)
.\raaaaah-weeee.exe 4              # 156 fenêtres max
.\raaaaah-weeee.exe --infinite     # non borné : VM uniquement
.\raaaaah-weeee.exe --sound C:\sons\raaaaah.mp3
.\raaaaah-weeee.exe --uninstall    # retire une ancienne copie du dossier Démarrage
```

Le binaire est en sous-système `windows` (pas de console) : tuer le processus
depuis le gestionnaire de tâches si besoin.

## Nettoyage après une ancienne version

Les versions précédentes pouvaient se copier dans le dossier Démarrage de
l'utilisateur avec `--startup`. Ce comportement a été retiré ; `--uninstall`
supprime la copie éventuellement laissée dans :

```
%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\
```

## Fichiers

- `src/main.rs` — tout le code.
- `Cargo.toml` — `windows-sys` (fenêtres, GDI, MCI).
- `AAAAAHWEEEE.mp3` — le son, lu à côté de l'exe (pas embarqué dans le binaire).
