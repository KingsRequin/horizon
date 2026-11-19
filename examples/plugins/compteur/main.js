// Un compteur par exemplaire du widget (chacun a son pas dans ses réglages).
const valeurs = {};

function montrer(inst) {
  horizon.ui.set("valeur", valeurs[inst] || 0, { instance: inst });
}

horizon.ui.on("ajouter", (inst) => {
  valeurs[inst] = (valeurs[inst] || 0) + horizon.settings.get("pas", inst);
  montrer(inst);
});

horizon.ui.on("zero", (inst) => {
  valeurs[inst] = 0;
  montrer(inst);
  horizon.log.info("compteur remis à zéro", inst);
});

for (const inst of horizon.instances()) montrer(inst);
horizon.on("instances", (list) => list.forEach(montrer));
