# Lecture des notifications

Horizon lit les notifications Windows des autres applis avec `UserNotificationListener`
(`crates/horizon-sys/src/notifications.rs`). Cette API exige une **identité de paquet**.

## Identité de paquet (paquet « sparse »)

- `natif/horizon.exe.manifest` (embarqué par `natif/build.rs`) déclare l'élément
  `<msix publisher="CN=Horizon" packageName="Horizon" applicationId="Horizon"/>`.
  Le manifeste doit rester en **ASCII** : un caractère accentué le rend illisible pour Windows
  (« configuration côte à côte incorrecte », journal SideBySide « syntaxe XML non valide ligne 1 »).
- `packaging/AppxManifest.xml` : paquet sans fichiers (`AllowExternalContent`), capacité
  `userNotificationListener`, exécutable externe `horizon.exe`.
- `packaging/register-dev.ps1 -ExeDir <dossier de horizon.exe>` enregistre le paquet avec
  `Add-AppxPackage -Register ... -ExternalLocation ...`. Le **mode développeur** de Windows étant actif
  sur ce PC, **ni signature ni droits administrateur** ne sont nécessaires (vérifié le 10 novembre 2026).
  `-Unregister` retire le paquet. Ré-enregistrer si le dossier de `horizon.exe` change.

## Fonctionnement

- L'accès est demandé au premier lancement (`RequestAccessAsync`) ; il a été accordé sans invite.
  S'il est refusé : Paramètres Windows → Confidentialité → Notifications → autoriser Horizon.
- `NotificationChanged` n'est pas utilisable par une appli de bureau : la liste est relue toutes les 2 s
  et comparée à la précédente (`Added` / `Removed`).
- Diagnostic : `horizon.exe --notif-probe` affiche pendant 60 s ce qui est lu (lancer depuis
  PowerShell ; Git Bash refuse d'exécuter un exe doté d'une identité de paquet).

## Contournement (non nécessaire à ce jour)

Si l'identité de paquet cessait de fonctionner : plugin BetterDiscord qui pousse les messages entrants
à Horizon par WebSocket local (même principe que KrispBridge), avec la même interface `NotifEvent`.
