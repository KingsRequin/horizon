//! Modèles de batterie des casques Logitech qui ne donnent qu'une tension (fonctionnalités
//! HID++ 0x1001 et 0x1F20) : tension → pourcentage.
//!
//! G HUB a une fiche par appareil (`…_battery.xml`) : décharge mesurée minute par minute
//! (tension, énergie sortie) et charge (tension, pourcentage). En décharge, le pourcentage est
//! la part d'énergie restante. Les tables ci-dessous n'en gardent que les points utiles :
//! interpolées, elles restent à moins d'un point des mesures.
//!  * `GENERIC` : courbe de Solaar, reprise par Halo Battery ; elle coïncide avec la fiche d'une
//!    souris (046D:407F). Elle sert aux casques dont la fiche manque, en charge comme en
//!    décharge. Sur le PRO X sans fil, elle donnait 7 à 17 points de moins que G HUB en
//!    décharge, et beaucoup trop en charge (82 % à 4,0 V, pour 21 % réels) ;
//!  * `PRO_X` : fiche du PRO X sans fil (046D:0ABA).
//!
//! En charge, la tension monte jusqu'à un sommet (87 % sur le PRO X) puis redescend pendant que
//! la charge se termine : une même tension se lit donc de deux façons, et c'est le sommet
//! atteint depuis le début de la charge qui les départage. Un casque déjà en fin de charge à la
//! première lecture est lu sur la courbe montante (77 à 86 % au lieu de 88 à 100 %) jusqu'à ce
//! que la charge soit finie.

/// Points (mV, %) de la plus haute à la plus basse tension, à interpoler.
type Curve = &'static [(u16, u8)];

/// Modèle de batterie d'un casque.
pub struct Model {
    /// Hors charge.
    discharge: Curve,
    /// En charge ; `None` : fiche inconnue, la courbe de décharge sert aussi en charge.
    charge: Option<Charge>,
}

struct Charge {
    /// Tension montante ; le premier point est le sommet.
    rising: Curve,
    /// Fin de charge : la tension redescend du sommet pendant que le pourcentage finit de monter.
    topping: Curve,
}

/// Écart au sommet de la courbe montante en deçà duquel il est tenu pour atteint.
const PEAK_MARGIN_MV: u16 = 10;

static GENERIC: Model = Model {
    discharge: &[
        (4186, 100),
        (4067, 90),
        (3989, 80),
        (3922, 70),
        (3859, 60),
        (3811, 50),
        (3778, 40),
        (3751, 30),
        (3717, 20),
        (3671, 10),
        (3646, 5),
        (3579, 2),
        (3500, 0),
    ],
    charge: None,
};

static PRO_X: Model = Model {
    discharge: &[
        (4102, 100),
        (4038, 94),
        (3966, 87),
        (3819, 67),
        (3781, 58),
        (3756, 49),
        (3743, 43),
        (3741, 42),
        (3716, 33),
        (3687, 25),
        (3661, 20),
        (3654, 17),
        (3644, 15),
        (3478, 7),
        (3293, 0),
    ],
    charge: Some(Charge {
        rising: &[
            (4275, 87),
            (4210, 78),
            (4139, 65),
            (4084, 52),
            (4051, 41),
            (4034, 33),
            (4021, 26),
            (4001, 20),
            (3917, 6),
            (3809, 3),
            (3444, 0),
        ],
        topping: &[(4273, 87), (4219, 97), (4207, 100)],
    }),
};

/// Modèles connus, par identifiant produit.
static MODELS: &[(u16, &Model)] = &[(0x0ABA, &PRO_X)];

/// Modèle de batterie d'un casque ; le modèle générique si sa fiche manque.
pub fn model(pid: u16) -> &'static Model {
    MODELS.iter().find(|(p, _)| *p == pid).map_or(&GENERIC, |(_, m)| m)
}

impl Model {
    /// Pourcentage hors charge.
    pub fn discharging(&self, mv: u16) -> u8 {
        percent(self.discharge, mv)
    }

    /// Pourcentage en charge ; `peak` : plus haute tension lue depuis le début de cette charge
    /// (celle-ci comprise).
    pub fn charging(&self, mv: u16, peak: u16) -> u8 {
        match &self.charge {
            None => percent(self.discharge, mv),
            Some(c) if peak.saturating_add(PEAK_MARGIN_MV) >= c.rising[0].0 => percent(c.topping, mv),
            Some(c) => percent(c.rising, mv),
        }
    }
}

/// Interpolation sur une courbe, bornée à ses extrémités (arrondi au pair, comme Python).
fn percent(curve: Curve, mv: u16) -> u8 {
    if mv >= curve[0].0 {
        return curve[0].1;
    }
    for w in curve.windows(2) {
        let ((hi_mv, hi_p), (lo_mv, lo_p)) = (w[0], w[1]);
        if mv >= lo_mv {
            let slope = (f64::from(hi_p) - f64::from(lo_p)) / f64::from(hi_mv - lo_mv);
            return (f64::from(lo_p) + f64::from(mv - lo_mv) * slope).round_ties_even() as u8;
        }
    }
    curve[curve.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tensions strictement décroissantes, pourcentages dans le sens `rising` (ou l'inverse).
    fn well_formed(curve: Curve, rising: bool) {
        assert!(curve.len() >= 2);
        for w in curve.windows(2) {
            assert!(w[0].0 > w[1].0, "tensions : {:?} puis {:?}", w[0], w[1]);
            assert!(if rising { w[0].1 > w[1].1 } else { w[0].1 < w[1].1 }, "pourcentages : {:?} puis {:?}", w[0], w[1]);
        }
        assert!(curve.iter().all(|&(_, p)| p <= 100));
    }

    #[test]
    fn curves_are_well_formed() {
        for m in [&GENERIC, &PRO_X] {
            well_formed(m.discharge, true);
            assert_eq!((m.discharge[0].1, m.discharge[m.discharge.len() - 1].1), (100, 0));
            if let Some(c) = &m.charge {
                well_formed(c.rising, true);
                well_formed(c.topping, false);
                assert_eq!(c.rising[c.rising.len() - 1].1, 0);
                assert_eq!(c.topping[0].1, c.rising[0].1, "la fin de charge part du sommet");
                assert_eq!(c.topping[c.topping.len() - 1].1, 100);
            }
        }
    }

    #[test]
    fn generic_curve() {
        let m = model(0x0AB5);
        assert_eq!(m.discharging(4300), 100);
        assert_eq!(m.discharging(4186), 100);
        assert_eq!(m.discharging(3811), 50);
        assert_eq!(m.discharging(3500), 0);
        assert_eq!(m.discharging(3000), 0);
        // À mi-chemin entre 3778 (40 %) et 3811 (50 %) : 44,8 → 45.
        assert_eq!(m.discharging(3794), 45);
        // Sans fiche : la même courbe en charge, quel que soit le sommet.
        assert_eq!(m.charging(3811, 3811), 50);
        assert_eq!(m.charging(3811, 4300), 50);
    }

    #[test]
    fn pro_x_discharge_follows_the_g_hub_sheet() {
        let m = model(0x0ABA);
        // Tensions de la fiche aux paliers d'énergie restante.
        for (mv, level) in [(4102, 100), (3993, 90), (3915, 80), (3758, 50), (3661, 20), (3548, 10), (3293, 0)] {
            assert_eq!(m.discharging(mv), level, "{mv} mV");
        }
        assert_eq!(m.discharging(4200), 100);
        assert_eq!(m.discharging(3000), 0);
        // La courbe générique lisait 33 % là où G HUB en affiche 50.
        assert_eq!(GENERIC.discharging(3758), 33);
    }

    #[test]
    fn pro_x_charge_rises_then_tops_off() {
        let m = model(0x0ABA);
        // Tension montante (fiche : 4,056 V → 43 %, 4,139 V → 65 %).
        assert_eq!(m.charging(4056, 4056), 43);
        assert_eq!(m.charging(4139, 4139), 65);
        assert_eq!(m.charging(3300, 3300), 0);
        // La même tension avant et après le sommet (fiche : 84 % puis 91 %).
        assert_eq!(m.charging(4250, 4250), 84);
        assert_eq!(m.charging(4250, 4275), 91);
        // Sommet tenu pour atteint à 10 mV près.
        assert_eq!(m.charging(4264, 4264), 85);
        assert_eq!(m.charging(4265, 4265), 88);
        // Charge finie : la tension est retombée sous la fin de la courbe.
        assert_eq!(m.charging(4190, 4275), 100);
    }
}
