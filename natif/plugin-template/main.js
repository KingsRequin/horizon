// Logique du plugin (objet `horizon`, voir PLUGINS.md). Un seul script pour tous les
// exemplaires du widget : chacun a son identifiant (`inst`).
const clics = {};

function montrer(inst) {
  horizon.ui.set("texte", horizon.settings.get("texte", inst), { instance: inst });
  horizon.ui.set("clics", clics[inst] || 0, { instance: inst });
}

horizon.ui.on("clique", (inst) => {
  clics[inst] = (clics[inst] || 0) + 1;
  montrer(inst);
});
horizon.ui.on("zero", (inst) => {
  clics[inst] = 0;
  montrer(inst);
});
horizon.settings.onChange((inst) => montrer(inst));
for (const inst of horizon.instances()) montrer(inst);
horizon.on("instances", (liste) => liste.forEach(montrer));
