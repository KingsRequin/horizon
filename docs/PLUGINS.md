# Écrire un plugin pour Horizon

Ce guide est écrit pour être donné tel quel à une IA (Claude, ChatGPT…) : il contient tout le
contrat d'un plugin. Les humains peuvent le lire aussi.

> **API en préversion** : elle peut encore changer. `"api"` est le niveau dont le plugin a
> besoin : `0` pour le socle, `1` s'il se sert d'un ajout marqué « niveau 1 » ci-dessous. Une
> version d'Horizon plus ancienne ne propose pas un plugin d'un niveau qu'elle ne connaît pas.

## Prompt à coller en tête de conversation

```text
Tu vas écrire un plugin pour Horizon, une barre d'outils pour Windows. Le contrat complet est
dans le document PLUGINS.md ci-dessous : respecte-le à la lettre (noms de fichiers, composants
`Widget`/`Panel` qui héritent de `Window`, API `horizon`, permissions). N'invente aucune
fonction qui n'y figure pas. Donne-moi chaque fichier en entier (plugin.json, widget.slint,
panel.slint si besoin, main.js si besoin). Ensuite, je lancerai
`horizon.exe --check-plugin <dossier>` et je te recollerai les erreurs éventuelles.

Ce que je veux : <décris ton widget ici>

<colle ici tout le contenu de PLUGINS.md>
```

## 1. Ce qu'est un plugin

Un dossier `%APPDATA%\Horizon\plugins\<id>\` (le nom du dossier est l'`id` du plugin) :

| Fichier | Rôle | Obligatoire |
|---|---|---|
| `plugin.json` | manifeste : nom, version, widget, panneau, permissions, réglages | oui |
| `widget.slint` | le widget de la barre (`export component Widget inherits Window`) | si `"widget"` |
| `panel.slint` | le panneau ouvert au survol (`export component Panel inherits Window`) | si `"panel": true` |
| `main.js` | la logique (JavaScript, objet `horizon`) | non |
| `assets/…` | images (PNG, JPEG) | non |

- Horizon voit le dossier en moins d'une seconde, sans redémarrer, et recharge le plugin à
  chaque modification de `plugin.json`, `main.js`, d'un `.slint` ou de `assets/`.
- On l'ajoute à une barre en mode édition (*Ajouter un widget*, comme les autres widgets). Le bouton *Nouveau plugin* de la page *État* crée un modèle et le pose
  sur la barre principale.
- Vérification hors d'Horizon : `horizon.exe --check-plugin <dossier>` (manifeste, Slint,
  syntaxe du script ; code de sortie 0 si tout va bien, sinon une erreur par ligne avec le
  fichier et la ligne).
- Une erreur au chargement s'affiche dans la barre : pastille rouge à la place du widget, le
  détail au survol (fichier:ligne). La page *Réglages d'Horizon › Plugins* montre une tuile par
  plugin : son état, son coût et sa dernière erreur.
- Une nouvelle version de `main.js` qui ne se charge pas laisse l'ancienne tourner ; une
  interface `.slint` qui ne compile plus laisse l'ancienne affichée.

## 2. `plugin.json`

```json
{
  "id": "meteo-locale",
  "name": "Météo locale",
  "version": "1.0.0",
  "api": 0,
  "author": "Ton nom",
  "description": "Une phrase sur ce que fait le plugin.",
  "homepage": "https://github.com/toi/meteo-locale",
  "icon": "cloudSun",
  "widget": { "squeeze": 5, "grows": null, "clickableOnHover": false },
  "panel": true,
  "permissions": {
    "connect": ["ws://127.0.0.1:28650"],
    "http": ["https://api.example.com/v1/"],
    "files": ["%LOCALAPPDATA%/MonAppli/etat.json"],
    "events": ["dnd", "media"],
    "system": ["probe"]
  },
  "settings": [
    { "key": "ville", "type": "string", "label": "Ville", "default": "Paris" },
    { "key": "unite", "type": "choice", "label": "Unité", "default": "c",
      "options": [{ "value": "c", "label": "°C" }, { "value": "f", "label": "°F" }] },
    { "key": "pas", "type": "number", "label": "Pas", "default": 1, "min": 1, "max": 10 },
    { "key": "details", "type": "bool", "label": "Afficher le détail", "default": true },
    { "key": "jeton", "type": "secret", "label": "Clé d'accès", "help": "Créée sur la page Compte du service." },
    { "key": "lieux", "type": "list", "label": "Autres lieux", "fields": [
      { "key": "nom", "type": "string", "label": "Nom" },
      { "key": "favori", "type": "bool", "label": "Favori" }
    ] }
  ]
}
```

| Champ | Règle |
|---|---|
| `id` | 1 à 40 caractères parmi `a-z`, `0-9`, `_`, `-` ; égal au nom du dossier ; différent des widgets d'Horizon (`clock`, `media`, `weather`…) |
| `name`, `version` | obligatoires (texte) |
| `api` | `0`, ou `1` si le plugin se sert d'un ajout du niveau 1 |
| `author`, `description`, `homepage` | facultatifs |
| `icon` | nom d'une icône d'Horizon (liste au §5) ; à défaut `squaresFour` |
| `widget` | présent : le plugin a un widget de barre. `squeeze` : rang de masquage quand la barre manque de place (1 = masqué le premier) ; `grows` : le widget grandit avec son contenu (sa plus petite largeur en px) ; `clickableOnHover` : reste cliquable sur une barre « survol seul » |
| `panel` | `true` : le plugin a un panneau (`panel.slint`) |
| `permissions` | tout ce que le script joint (§7) ; tout champ inconnu est une erreur |
| `settings` | réglages de chaque exemplaire du widget, page de réglages générée par Horizon |

Types de réglage : `bool`, `number` (`min`, `max`, `step` ; la page de réglages ne propose que
des entiers), `string`, `choice` (`options` : chaînes ou `{ "value", "label" }`), `color`
(`#rrggbb`), `secret` (gardé dans le Gestionnaire d'identifiants de Windows, jamais dans un
fichier). `key` : lettres, chiffres, `_`. `default` doit convenir au type (sinon : défaut du
type : `false`, `min` ou 0, `""`, première option, `#ffffff`, `[]`).

Niveau 1 :

- `help` (facultatif, tout type) : une ligne d'aide sous le libellé.
- `list` : une liste que l'utilisateur allonge et raccourcit (serveurs, villes, comptes…).
  `fields` déclare les champs de chaque élément, avec les types `bool`, `number`, `string`,
  `choice` et `color` (ni `secret`, ni `list`). La valeur est un tableau d'objets, 64 au plus,
  chaque champ toujours présent : `horizon.settings.get("lieux")` → `[{ nom: "Lyon", favori: false }]`.
  La page montre chaque élément sous le nom de son premier champ `string` rempli.

## 3. Le widget (`widget.slint`)

Slint 1.18 (https://slint.dev/releases/1.18.1/docs/slint/). Le fichier exporte un composant
**`Widget` qui hérite de `Window`** :

```slint
import { Island, Icon, Pill, Theme } from "@horizon";

export component Widget inherits Window {
    // Valeurs posées par le script (horizon.ui.set) : des `in property`.
    in property <string> texte: "…";
    in property <int> nombre;
    // Rappels vers le script (horizon.ui.on).
    callback clique();
    // Le widget garde ses clics (sinon un clic épingle son panneau).
    out property <bool> own-clicks: true;
    background: transparent;

    VerticalLayout {
        alignment: center;
        Island {
            Icon { name: "cloudSun"; y: (parent.height - self.height) / 2; }
            Text { text: root.texte; color: Theme.text; font-size: Theme.fs-body; vertical-alignment: center; }
            Pill { text: root.nombre; y: (parent.height - self.height) / 2; }
        }
    }
    TouchArea {
        mouse-cursor: pointer;
        clicked => { root.clique(); }
    }
}
```

- **Taille** : la hauteur est celle de la bande de la barre (40 px environ) ; la largeur est
  la largeur voulue par le contenu (`preferred-width`). 1 200 × 200 px au plus. Ne pas fixer
  `width` sur le `Window`.
- **Fond** : `background: transparent;` (l'îlot `Island` dessine le fond habituel des widgets).
- **Centrage vertical** dans un `Island` : `y: (parent.height - self.height) / 2;` sur chaque
  élément de hauteur fixe (icône, pastille, jauge), ou un `VerticalLayout { alignment: center; }`.
- **Propriétés posées par Horizon**, si le composant les déclare (`in property`) :
  `compact: bool` (barre resserrée), `bar-height: length`, `limit: length` (largeur maximale
  d'un widget qui grandit, `"grows"`), `hovered: bool`, `instance: string` (identifiant de
  l'exemplaire).
- **Clics** : sans `own-clicks: true`, un clic sur le widget épingle son panneau (s'il en a un).
  Avec, les clics vont au widget seulement. Le survol ouvre toujours le panneau.
- **Animations** Slint (`animate`, `Timer`) : permises. Le widget est redessiné au plus 20 fois
  par seconde, et seulement quand il change.

## 4. Le panneau (`panel.slint`)

```slint
import { PanelBase, ListRow, Theme } from "@horizon";

export component Panel inherits Window {
    in property <string> resume;
    callback actualiser();
    background: transparent;

    PanelBase {
        title: "Météo locale";
        Text { text: root.resume; color: Theme.text; font-size: Theme.fs-body; wrap: word-wrap; }
        ListRow { icon: "arrowsClockwise"; title: "Actualiser"; clicked => { root.actualiser(); } }
    }
}
```

- Ouvert au survol du widget, épinglé au clic ; aussi montré en tuile sur une barre haute.
- 900 × 1 200 px au plus ; sa taille est celle voulue par son contenu.
- Le panneau et le widget d'un même exemplaire partagent le script, et le même identifiant
  d'exemplaire.
- Le clavier va au panneau quand il a le focus (`FocusScope`, `TextInput`).
- Le panneau n'existe que tant qu'il est montré. Pour rafraîchir des données à son ouverture
  puis régulièrement, y poser un minuteur qui appelle le script :
  `callback tic(); Timer { interval: 1s; running: true; triggered => { root.tic(); } }` et
  `horizon.ui.on("tic", (instance) => { … })`.

## 5. La bibliothèque `@horizon`

`import { Theme, Island, Icon, Pill, IconText, Gauge, ListRow, PanelBase } from "@horizon";`

**`Theme`** (global, suit le style des barres et la nuit) : `graphic` (style simple, sans îlot),
`font`, `font-mono`, couleurs `text`, `text-dim`, `accent`, `ok`, `warn`, `bad`, `info`,
`island-bg`, `island-border`, `popup-bg`, `hover`, `press`, `track`, `divider` ; tailles
`island-h`, `island-radius`, `inner-h`, `inner-radius`, `fs-meta`, `fs-body`, `fs-title`.

| Composant | Propriétés |
|---|---|
| `Icon` | `name` (nom ci-dessous), `size` (16px), `color` (`Theme.text`) |
| `Island` | `padding-x` (12px), `gap` (10px) ; enfants posés en ligne, centrés |
| `Pill` | `text`, `tint` (`Theme.accent`) : petite étiquette arrondie |
| `IconText` | `icon`, `text`, `color` |
| `Gauge` | `value` (0 à 1), `tint` : jauge horizontale de 36 × 6 px |
| `ListRow` | `icon`, `title`, `subtitle`, `callback clicked()` : ligne de panneau, survol et clic |
| `PanelBase` | `title` ; colonne avec marges (260 px de large au moins) |

**Icônes** (Phosphor) : `appWindow arrowCounterClockwise arrowSquareIn arrowSquareOut
arrowsClockwise atom batteryFull bellSimple bellSlash browsers calendarBlank camera caretDown
caretDownFill caretRightFill chartDonut check checkCircle clock cloud cloudFog cloudLightning
cloudMoon cloudRain cloudSnow cloudSun coins cpu deviceMobile discordLogo drop earSlash
fastForward fileText folderSimple gauge gearSix gitBranch globeSimple graphicsCard hardDrives
headphones headset keyboard lightbulb lightbulbFill lightning magnifyingGlass mapPin memory
microphone microphoneFill microphoneSlash moon moonFill musicNotes pause pencilSimple phoneCall
phoneDisconnect phoneIncoming play plus power pulse pushPin pushPinFill rewind shieldWarning
signOut skipBack skipForward sparkle speakerHigh speakerLow speakerSlash squaresFour stop sun
terminalWindow timer trash userSwitch users videoCamera warning watch waveform wind x`.

**Interdit dans les `.slint`** (refusé au chargement) : `@image-url(…)`, l'import de polices
(`.ttf`, `.otf`…), et tout import hors du dossier du plugin ou d'une autre bibliothèque que
`@horizon` et `std-widgets.slint`. Une image s'affiche par une propriété `in property <image>`
que le script remplit avec le chemin d'un fichier du dossier :
`horizon.ui.set("photo", "assets/photo.png")` (PNG ou JPEG, 4 096 px et 8 Mo au plus).

## 6. Le script (`main.js`)

JavaScript moderne (QuickJS : ES2020 et plus) exécuté en mode strict. **Rien d'autre que l'objet
`horizon`** : ni `require`, ni `import`, ni `fetch`, ni `setTimeout`, ni `window`, ni
`process`. Chaque appel du script (chargement, rappel, minuteur) a **50 ms** au plus ; la
mémoire est limitée à **32 Mo**.

Un seul script tourne par plugin, quel que soit le nombre de ses widgets sur les barres :
chaque exemplaire a un identifiant (`instance`).

### Interface

| Appel | Effet |
|---|---|
| `horizon.ui.set(prop, valeur)` | pose la propriété `prop` (déclarée `in property` dans `Widget` ou `Panel`) sur **tous** les exemplaires |
| `horizon.ui.set(prop, valeur, { instance })` | sur un seul exemplaire |
| `horizon.ui.on(nom, (instance, ...args) => {})` | le `callback nom(...)` du `Widget` ou du `Panel` appelle cette fonction, avec l'exemplaire en premier argument |

Conversion des valeurs : nombres → `int`/`float`/`length` (px) ; chaînes → `string` ;
booléens → `bool` ; `"#rrggbb"` ou `"#rrggbbaa"` → `color`/`brush` ; chemin d'un fichier du
dossier → `image` ; tableau → modèle d'un `for` (`[Struct]`), 1 000 éléments au plus ; objet →
`struct`. **Dans un tableau ou un objet**, une chaîne `#rrggbb` devient une couleur et un
nom de champ `nom_avec_tirets` devient `nom-avec-tirets` ; une image n'y est pas possible.
Une valeur posée est rejouée sur les exemplaires créés ensuite (panneau ouvert plus tard).

### Exemplaires et réglages

| Appel | Effet |
|---|---|
| `horizon.instances()` | tableau des identifiants des exemplaires |
| `horizon.on("instances", (liste) => {})` | exemplaires ajoutés ou retirés |
| `horizon.settings.get(cle, instance?)` | valeur d'un réglage (défaut compris ; sans `instance` : le premier exemplaire) |
| `horizon.settings.onChange((instance, reglages) => {})` | réglages d'un exemplaire changés |
| `horizon.settings.open(instance?)` | ouvre la page de réglages de l'exemplaire (sans `instance` : le premier) ; une par seconde — niveau 1 |

### Minuteurs, journal, fin

| Appel | Effet |
|---|---|
| `horizon.timer.every(ms, fn)` → `id` | répète `fn` toutes les `ms` (250 ms au moins, 24 h au plus ; 32 minuteurs au plus) |
| `horizon.timer.clear(id)` | arrête un minuteur |
| `horizon.log.info(...)`, `warn`, `error` | journal d'Horizon (20 lignes par seconde au plus) |
| `horizon.on("unload", () => {})` | Horizon se ferme ou le plugin est rechargé (dire au revoir à une application) |

### Îlot, alertes, palette, liens

| Appel | Effet |
|---|---|
| `horizon.island.show({ id, icon, title, subtitle, since, severity })` | activité durable dans l'îlot (comme un appel) : `since` (ms Unix) ajoute un chronomètre, `severity: "attention"` la met en avant ; 8 par plugin |
| `horizon.island.hide(id)` | la retire |
| `horizon.alert({ title, body, icon })` | alerte passagère (une toutes les 5 s au plus ; masquée pendant « Ne pas déranger ») |
| `horizon.palette.set([{ id, label, icon }])` | entrées « <nom du plugin> : <label> » dans la palette de commandes (50 au plus) |
| `horizon.palette.on((id) => {})` | une entrée a été choisie |
| `horizon.open(url)` | ouvre un lien `https://` dans le navigateur (adresse déclarée dans `permissions.http` ; un par seconde). Avec l'action `open` (§7, niveau 1) : tout lien `http://` ou `https://` |

### Applications, web, fichiers (sous permissions)

| Appel | Effet |
|---|---|
| `const c = horizon.connect("ws://127.0.0.1:28650")` | connexion WebSocket locale, rouverte seule si elle tombe ; 8 au plus |
| `c.onMessage((texte) => {})` | message reçu (texte ; `JSON.parse` au besoin) |
| `c.onState((etat) => {})` | `"open"` ou `"closed"` |
| `c.send(valeur)` | envoie une chaîne, ou un objet en JSON (1 Mo, 64 en attente au plus) |
| `c.close()` | ferme |
| `await horizon.http.get(url, { headers })` | `{ status, text }` ; `https://` déclaré, sans redirection, 10 s, 2 Mo |
| `await horizon.http.post(url, corps, { headers })` | idem avec un corps (chaîne, ou objet en JSON) |
| `horizon.files.read(chemin)` | contenu d'un fichier texte déclaré (1 Mo au plus) ou `null` |
| `horizon.files.watch(chemin, (texte) => {})` | relit le fichier quand il change (toutes les secondes ; 8 au plus) |

En-têtes HTTP permis : `Accept`, `Accept-Language`, `Authorization`, `Content-Type`,
`If-None-Match`, `If-Modified-Since`, les `X-…`, et au niveau 1 `anthropic-beta` et
`anthropic-version`. Un appel non déclaré lève une erreur « permission manquante ».

### Machines et terminal (`permissions.system`, niveau 1)

| Appel | Effet |
|---|---|
| `const r = await horizon.net.probe(cible)` | la machine répond-elle ? `{ ok, ms, error }` (`ms` : latence si `ok`, `error` : texte sinon ; jamais rejetée). `cible` : `"hôte:port"` (connexion TCP, 1,5 s) ou une adresse `http://` / `https://` (requête `HEAD`, 4 s, certificat non vérifié, toute réponse HTTP vaut `ok`). Rien n'est lu de la machine. Action `probe` ; 16 requêtes en cours au plus, `http` compris |
| `horizon.ssh.open(cible)` | ouvre un terminal SSH (Windows Terminal, sinon une console) vers `cible` : un alias de `~/.ssh/config`, un hôte ou `utilisateur@hôte` (lettres, chiffres, `. - _ @ : [ ]`). Action `ssh` ; un par seconde |

### Événements d'Horizon (`permissions.events`)

`horizon.on(nom, (donnees) => {})` ; la dernière valeur est donnée tout de suite au
chargement (sauf `alert` et `timer`).

| Nom | Données |
|---|---|
| `accent` | `{ color }` : couleur de la pochette en cours (sinon du fond d'écran), `#rrggbb` ou `null` |
| `media` | `{ title, artist, playing }` |
| `dnd` | `{ active, light }` : « Ne pas déranger » (et son option « lumière ») |
| `alert` | `{ kind, title, shown, light }` : une alerte d'Horizon (`claude`, `battery`, `server`, `timer`, `notification`, `feedback`, `update`) ; `shown` : montrée dans l'îlot ; `light` : signal lumineux voulu (réglage « Claude attend › Signal lumineux ») |
| `timer` | `{ state: "done", title }` : fin du minuteur ou changement de phase |
| `presence` | `{ away }` : session verrouillée ou écran éteint |
| `foreground` | `{ exe }` : application au premier plan |

## 7. Permissions et accord

- `connect` : adresses `ws://127.0.0.1:<port>` ou `ws://localhost:<port>` (préfixes : un
  chemin plus long est couvert).
- `http` : préfixes `https://hôte/chemin/` (même hôte, même port, chemin qui commence par
  celui déclaré). Sert aussi à `horizon.open`.
- `files` : chemins exacts, en lecture seule ; `%LOCALAPPDATA%`, `%APPDATA%` et, au niveau 1,
  `%USERPROFILE%` permis en tête ; ni `..`, ni chemin réseau.
- `events` : noms du §6.
- `system` (niveau 1) : actions sur le système, par leur nom. `probe` : tester si des machines
  répondent (`horizon.net.probe`) ; `ssh` : ouvrir un terminal SSH (`horizon.ssh.open`) ;
  `open` : ouvrir n'importe quel lien web (`horizon.open`).

L'utilisateur voit ces demandes en clair et doit les accepter (page *État* › *Autoriser*)
avant que le plugin se charge ; une nouvelle version qui en demande plus redemande l'accord.
Un plugin sans permission se charge sans accord. **Déclarer le strict nécessaire.**

## 8. Budget

| Limite | Effet |
|---|---|
| plus de 2 % d'un cœur en moyenne sur une minute, ou plus de 50 % sur 5 s | plugin désactivé jusqu'au prochain lancement d'Horizon (alerte, *Réactiver* sur la page État) |
| 3 plantages de script en une minute (exception non rattrapée, appel de plus de 50 ms, mémoire) | désactivé |
| création ou rendu de l'interface de plus de 250 ms | compté comme un plantage |
| plus de 1 000 messages vers l'interface en attente | compté comme un plantage |

Ce qui est compté : le temps processeur du script, du rendu de son interface et de ce qu'Horizon
fait pour ses messages (valeurs, îlot, palette…), pas les attentes (PC chargé, pilote graphique
figé). Une valeur reposée identique ne coûte rien (pas de rendu). Le journal donne les mesures quand un plugin est coupé. Un plugin absent de toutes les
barres ne coûte rien (rien n'est chargé). Bonnes pratiques : préférer `files.watch` et les
connexions à des minuteurs rapides ; garder les calculs courts.

## 9. Pièges connus

- La largeur du widget vient de sa mise en page : le contenu du `Window` doit être dans un
  `VerticalLayout` ou un `HorizontalLayout`. Un `if` posé directement sous le `Window` n'a pas de
  largeur (le widget reste invisible) : le mettre **dans** la mise en page
  (`VerticalLayout { if cond: Island { … } }`).
- Dans un `Rectangle` simple, un enfant est centré par défaut : `x: 0;` pour le coller à
  gauche.
- Un `TouchArea` déclaré après d'autres éléments les recouvre et prend leurs clics : le placer
  à la fin seulement s'il doit couvrir tout le widget.
- Jamais d'adoucissement qui dépasse (`ease-out-back`, `cubic-bezier(…, 1.2)`) sur une
  **opacité** : le rendu logiciel déborde.
- Les noms Slint `ma_propriete` et `ma-propriete` sont les mêmes ; côté script, utiliser le
  nom avec tirets : `horizon.ui.set("ma-propriete", …)`.
- Un `in property <length>` reçoit des pixels : `horizon.ui.set("largeur", 120)`.
- L'état d'une propriété Slint est perdu au rechargement de l'interface : garder l'état dans
  le script et le reposer (il est rejoué automatiquement).

## 10. Exemple complet : un compteur

`plugin.json`

```json
{
  "id": "compteur",
  "name": "Compteur",
  "version": "1.0.0",
  "api": 0,
  "description": "Un compteur par exemplaire, remis à zéro depuis son panneau.",
  "icon": "plus",
  "widget": { "squeeze": 8 },
  "panel": true,
  "settings": [
    { "key": "pas", "type": "number", "label": "Pas du compteur", "default": 1, "min": 1, "max": 10 }
  ]
}
```

`widget.slint`

```slint
import { Island, Icon, Pill } from "@horizon";

export component Widget inherits Window {
    in property <int> valeur;
    out property <bool> own-clicks: true;
    callback ajouter();
    background: transparent;
    VerticalLayout {
        alignment: center;
        Island {
            Icon { name: "plus"; y: (parent.height - self.height) / 2; }
            Pill { text: root.valeur; y: (parent.height - self.height) / 2; }
        }
    }
    TouchArea {
        mouse-cursor: pointer;
        clicked => { root.ajouter(); }
    }
}
```

`panel.slint`

```slint
import { PanelBase, ListRow, Theme } from "@horizon";

export component Panel inherits Window {
    in property <int> valeur;
    callback zero();
    background: transparent;
    PanelBase {
        title: "Compteur";
        Text { text: "Valeur : " + root.valeur; color: Theme.text; font-size: Theme.fs-body; }
        ListRow { icon: "arrowCounterClockwise"; title: "Remettre à zéro"; clicked => { root.zero(); } }
    }
}
```

`main.js`

```js
const valeurs = {};
const montrer = (inst) => horizon.ui.set("valeur", valeurs[inst] || 0, { instance: inst });

horizon.ui.on("ajouter", (inst) => {
  valeurs[inst] = (valeurs[inst] || 0) + horizon.settings.get("pas", inst);
  montrer(inst);
});
horizon.ui.on("zero", (inst) => {
  valeurs[inst] = 0;
  montrer(inst);
});
for (const inst of horizon.instances()) montrer(inst);
horizon.on("instances", (liste) => liste.forEach(montrer));
```

## 11. Exemple : relié à une application de ce PC

Une application locale (ici sur le port 28650) envoie son état en JSON et accepte des ordres.

`plugin.json` (extrait)

```json
{
  "id": "mon-appli",
  "name": "Mon appli",
  "version": "1.0.0",
  "api": 0,
  "widget": {},
  "permissions": { "connect": ["ws://127.0.0.1:28650"], "events": ["dnd"] }
}
```

`main.js`

```js
const appli = horizon.connect("ws://127.0.0.1:28650");
appli.onState((etat) => horizon.ui.set("connecte", etat === "open"));
appli.onMessage((texte) => {
  const msg = JSON.parse(texte);
  if (msg.event === "state") horizon.ui.set("texte", msg.label);
});
horizon.ui.on("basculer", () => appli.send({ cmd: "toggle" }));
// « Ne pas déranger » d'Horizon transmis à l'appli.
horizon.on("dnd", (d) => appli.send({ cmd: "quiet", on: d.active }));
horizon.on("unload", () => appli.send({ cmd: "bye" }));
```

## 12. Avant de rendre le plugin

1. `horizon.exe --check-plugin <dossier>` ne signale rien.
2. Le dossier porte le nom de l'`id` ; `"api": 0`, ou `1` si un ajout du niveau 1 sert.
3. Chaque `horizon.ui.set` vise une propriété déclarée `in property` ; chaque `horizon.ui.on`
   un `callback` déclaré.
4. Les permissions déclarées sont toutes utilisées, et rien n'est joint sans être déclaré.
5. Aucun minuteur plus rapide que nécessaire ; rien de lourd dans un rappel.
