//! Checking what an effect may hit against its restrictions (`restrict::Restr`): on the
//! battlefield, in graveyards and libraries, and for spells being cast.

use super::compile::*;
use super::resolve::Ctx;
use super::state::*;
use crate::abilities::Amount;
use crate::restrict::{self, Dyn, Obj, Restr};

impl<'a> Game<'a> {
    /// A permanent as a restriction sees it (`equipped` is looked up only when asked for).
    pub fn obj<'p>(&self, x: &'p Perm, need_equipped: bool) -> Obj<'p> {
        let probe = x.id == u32::MAX;
        Obj {
            types: x.types | if x.is_creature() { T_CREATURE } else { 0 },
            subtypes: &x.subtypes,
            mv: x.mv,
            colors: x.colors,
            power: x.p,
            toughness: x.t,
            kw: x.eff_kw,
            token: !probe && x.is_token(),
            tapped: x.tapped,
            attacking: x.attacking,
            blocking: x.blocking,
            counters: x.counters,
            equipped: need_equipped
                && !probe
                && self
                    .perms
                    .iter()
                    .any(|e| e.is(T_EQUIPMENT) && e.attached == Some(x.id)),
            entered: !probe && x.born == self.round && x.sick,
            damaged: x.damage > 0,
            dealt: x.dealt_damage,
        }
    }

    /// The values a restriction may need from the game: X, the source's power, the chosen
    /// creature type and colour of player `p` (computed only when asked for).
    pub fn dyn_values(&self, r: &Restr, ctx: Option<&Ctx>, p: usize, src: Option<u32>) -> Dyn {
        let mut d = Dyn {
            this_turn: true,
            ..Default::default()
        };
        if r.has(restrict::MV_LE_X) {
            d.x = Some(match ctx {
                Some(c) => self.x_value(c, &Amount { n: 3, x: true }),
                None => 3,
            });
        }
        if r.has(restrict::MV_LE_SRC_POWER) {
            let s = src
                .and_then(|id| self.idx(id))
                .map(|i| self.perms[i].p as i32);
            d.src_power = s.or_else(|| ctx.and_then(|c| c.snap.as_ref().map(|x| x.p as i32)));
        }
        if r.has(restrict::CHOSEN_TYPE | restrict::NOT_CHOSEN_TYPE) {
            d.chosen_type = self.chosen_type(p, src);
        }
        if r.has(restrict::CHOSEN_COLOR) {
            d.chosen_color = self.chosen_color(p);
        }
        d
    }

    /// "Choose a creature type": the one most of `p`'s other creatures share.
    pub fn chosen_type(&self, p: usize, src: Option<u32>) -> Option<u16> {
        let mut counts: Vec<(u16, u32)> = Vec::new();
        for x in self
            .perms
            .iter()
            .filter(|x| x.ctrl as usize == p && x.is_creature() && Some(x.id) != src)
        {
            for s in &x.subtypes {
                if let Some(id) = restrict::subtype_word(s) {
                    match counts.iter_mut().find(|c| c.0 == id) {
                        Some(c) => c.1 += 1,
                        None => counts.push((id, 1)),
                    }
                }
            }
        }
        counts.iter().max_by_key(|c| c.1).map(|c| c.0)
    }

    /// "Choose a color": the one most of `p`'s creatures have.
    pub fn chosen_color(&self, p: usize) -> u8 {
        (0..5)
            .map(|c| {
                let n = self
                    .perms
                    .iter()
                    .filter(|x| x.ctrl as usize == p && x.is_creature() && x.colors & (1 << c) != 0)
                    .count();
                (n, 1u8 << c)
            })
            .max_by_key(|x| x.0)
            .map(|x| x.1)
            .unwrap_or(0)
    }

    /// Does permanent `x` meet restriction `r`?
    pub fn perm_ok(&self, r: &Restr, x: &Perm, d: &Dyn) -> bool {
        r.is_empty() || r.matches(&self.obj(x, r.has(restrict::EQUIPPED)), d)
    }

    /// Does card `c` (in a graveyard, library or being cast) meet restriction `r`?
    pub fn card_ok(r: &Restr, c: &SimCard, d: &Dyn) -> bool {
        r.is_empty()
            || r.matches(
                &Obj {
                    types: c.types,
                    subtypes: &c.subtypes,
                    mv: c.mv,
                    colors: c.colors,
                    power: c.power,
                    toughness: c.toughness,
                    kw: c.kw,
                    token: false,
                    tapped: false,
                    attacking: false,
                    blocking: false,
                    counters: 0,
                    equipped: false,
                    entered: false,
                    damaged: false,
                    dealt: false,
                },
                d,
            )
    }

    /// What "it" / "that creature" / "those creatures" means for `t`: what the ability chose
    /// or made last, else (in a trigger about another creature) that creature. None: `t`
    /// doesn't refer back, or there's nothing to refer to.
    pub fn prev_ids(&self, ctx: &Ctx, t: &crate::abilities::Target) -> Option<Vec<u32>> {
        if !t.restr.has(restrict::PREV) {
            return None;
        }
        let alive: Vec<u32> = self
            .last_targets
            .iter()
            .copied()
            .filter(|&id| self.idx(id).is_some())
            .collect();
        if !alive.is_empty() || self.targeted {
            return Some(alive);
        }
        if t.restr.has(restrict::SUBJECT) {
            if let Some(s) = ctx
                .subject
                .filter(|&s| Some(s) != ctx.src && self.idx(s).is_some())
            {
                return Some(vec![s]);
            }
        }
        // "Draw a card for each creature you control with a +1/+1 counter on it. Those
        // creatures gain indestructible": the ones counted.
        if ctx
            .text
            .contains("for each creature you control with a +1/+1 counter on it")
        {
            return Some(
                self.perms
                    .iter()
                    .filter(|x| x.ctrl as usize == ctx.ctrl && x.is_creature() && x.counters > 0)
                    .map(|x| x.id)
                    .collect(),
            );
        }
        None
    }

    /// "With the greatest mana value/power among ...": keeps only each player's biggest.
    pub fn keep_greatest(&self, r: &Restr, cands: &mut Vec<usize>) {
        let key = |x: &Perm| -> i32 {
            if r.has(restrict::GREATEST_MV) {
                x.mv as i32
            } else {
                x.p as i32
            }
        };
        if !r.has(restrict::GREATEST_MV | restrict::GREATEST_POWER) {
            return;
        }
        let best: Vec<(u8, i32)> = cands
            .iter()
            .map(|&i| {
                let ctrl = self.perms[i].ctrl;
                let top = cands
                    .iter()
                    .filter(|&&j| self.perms[j].ctrl == ctrl)
                    .map(|&j| key(&self.perms[j]))
                    .max()
                    .unwrap_or(0);
                (ctrl, top)
            })
            .collect();
        let mut k = 0;
        cands.retain(|&i| {
            let keep = key(&self.perms[i]) >= best[k].1;
            k += 1;
            keep
        });
    }
}
