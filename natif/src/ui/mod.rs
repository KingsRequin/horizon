//! Interface Slint (`natif/ui/`) : fenêtres et leur liaison aux services. Tout ici vit sur le fil
//! de l'interface ; les services y envoient leurs changements par événements (`host::listen`).

pub mod accent;
pub mod bar;
pub mod bar_status;
pub mod dialogs;
pub mod edit;
pub mod essai;
pub mod format;
pub mod hint;
pub mod icons;
pub mod images;
pub mod island;
pub mod menu;
pub mod model;
pub mod options;
pub mod osd;
pub mod panel_hosts;
pub mod palette;
pub mod panels;
pub mod popup;
pub mod profiles;
pub mod pulse;
pub mod settings_window;
pub mod time;
pub mod tooltip;
pub mod widget_settings;

slint::include_modules!();
