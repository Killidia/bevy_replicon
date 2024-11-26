use bevy::{
    ecs::entity::{EntityHashMap, EntityHashSet},
    prelude::*,
    utils::hashbrown::hash_map::Entry,
};

use super::VisibilityPolicy;

/// Entity visibility settings for a client.
pub struct ClientVisibility {
    filter: VisibilityFilter,
}

impl ClientVisibility {
    /// Creates a new instance based on the preconfigured policy.
    pub(super) fn new(policy: VisibilityPolicy) -> Self {
        match policy {
            VisibilityPolicy::All => Self::with_filter(VisibilityFilter::All),
            VisibilityPolicy::Blacklist => Self::with_filter(VisibilityFilter::Blacklist {
                list: Default::default(),
                added: Default::default(),
                removed: Default::default(),
            }),
            VisibilityPolicy::Whitelist => Self::with_filter(VisibilityFilter::Whitelist {
                list: Default::default(),
                added: Default::default(),
                removed: Default::default(),
            }),
        }
    }

    /// Creates a new instance with a specific filter.
    fn with_filter(filter: VisibilityFilter) -> Self {
        Self { filter }
    }

    /// Resets the filter state to as it was after [`Self::new`].
    ///
    /// `cached_visibility` remains untouched.
    pub(super) fn clear(&mut self) {
        match &mut self.filter {
            VisibilityFilter::All => (),
            VisibilityFilter::Blacklist {
                list,
                added,
                removed,
            } => {
                list.clear();
                added.clear();
                removed.clear();
            }
            VisibilityFilter::Whitelist {
                list,
                added,
                removed,
            } => {
                list.clear();
                added.clear();
                removed.clear();
            }
        }
    }

    /// Updates list information and its sets based on the filter.
    ///
    /// Should be called after each tick.
    pub(crate) fn update(&mut self) {
        match &mut self.filter {
            VisibilityFilter::All => (),
            VisibilityFilter::Blacklist {
                list,
                added,
                removed,
            } => {
                // Remove all entities queued for removal.
                for entity in removed.drain() {
                    list.remove(&entity);
                }
                added.clear();
            }
            VisibilityFilter::Whitelist {
                list,
                added,
                removed,
            } => {
                // Change all recently added entities to `WhitelistInfo::Visible`
                // from `WhitelistInfo::JustVisible`.
                for entity in added.drain() {
                    list.insert(entity, WhitelistInfo::Visible);
                }
                removed.clear();
            }
        }
    }

    /// Removes a despawned entity tracked by this client.
    pub(super) fn remove_despawned(&mut self, entity: Entity) {
        match &mut self.filter {
            VisibilityFilter::All { .. } => (),
            VisibilityFilter::Blacklist {
                list,
                added,
                removed,
            } => {
                if list.remove(&entity).is_some() {
                    added.remove(&entity);
                    removed.remove(&entity);
                }
            }
            VisibilityFilter::Whitelist {
                list,
                added,
                removed,
            } => {
                if list.remove(&entity).is_some() {
                    added.remove(&entity);
                    removed.remove(&entity);
                }
            }
        }
    }

    /// Drains all entities for which visibility was lost during this tick.
    pub(super) fn drain_lost_visibility(&mut self) -> impl Iterator<Item = Entity> + '_ {
        match &mut self.filter {
            VisibilityFilter::All { .. } => VisibilityLostIter::AllVisible,
            VisibilityFilter::Blacklist { added, .. } => VisibilityLostIter::Lost(added.drain()),
            VisibilityFilter::Whitelist { removed, .. } => {
                VisibilityLostIter::Lost(removed.drain())
            }
        }
    }

    /// Sets visibility for a specific entity.
    ///
    /// Does nothing if the visibility policy for the server plugin is set to [`VisibilityPolicy::All`].
    pub fn set_visibility(&mut self, entity: Entity, visibile: bool) {
        match &mut self.filter {
            VisibilityFilter::All { .. } => {
                if visibile {
                    debug!(
                        "ignoring visibility enable due to {:?}",
                        VisibilityPolicy::All
                    );
                } else {
                    warn!(
                        "ignoring visibility disable due to {:?}",
                        VisibilityPolicy::All
                    );
                }
            }
            VisibilityFilter::Blacklist {
                list,
                added,
                removed,
            } => {
                if visibile {
                    // If the entity is already visibile, do nothing.
                    let Entry::Occupied(mut entry) = list.entry(entity) else {
                        return;
                    };

                    // If the entity was previously added in this tick, then undo it.
                    if added.remove(&entity) {
                        entry.remove();
                        return;
                    }

                    // For blacklisting an entity we don't remove the entity right away.
                    // Instead we mark it as queued for removal and remove it
                    // later in `Self::update`. This allows us to avoid accessing
                    // the blacklist's `removed` field in `Self::visibility_state`.
                    entry.insert(BlacklistInfo::QueuedForRemoval);
                    removed.insert(entity);
                } else {
                    // If the entity is already registered, reset its removal status.
                    if list.insert(entity, BlacklistInfo::Hidden).is_some() {
                        removed.remove(&entity);
                        return;
                    };

                    added.insert(entity);
                }
            }
            VisibilityFilter::Whitelist {
                list,
                added,
                removed,
            } => {
                if visibile {
                    // Similar to blacklist removal, we don't just add the entity to the list.
                    // Instead we mark it as `WhitelistInfo::JustAdded` and then set it to
                    // 'WhitelistInfo::Visible' in `Self::update`.
                    // This allows us to avoid accessing the whitelist's `added` field in
                    // `Self::visibility_state`.
                    if *list.entry(entity).or_insert(WhitelistInfo::JustAdded)
                        == WhitelistInfo::JustAdded
                    {
                        // Do not mark an entry as newly added if the entry was already in the list.
                        added.insert(entity);
                    }
                    removed.remove(&entity);
                } else {
                    // If the entity is not in the whitelist, do nothing.
                    if list.remove(&entity).is_none() {
                        return;
                    }

                    // If the entity was added in this tick, then undo it.
                    if added.remove(&entity) {
                        return;
                    }

                    removed.insert(entity);
                }
            }
        }
    }

    /// Checks if a specific entity is visible.
    pub fn is_visible(&self, entity: Entity) -> bool {
        match self.visibility_state(entity) {
            Visibility::Hidden => false,
            Visibility::Gained | Visibility::Visible => true,
        }
    }

    /// Returns visibility of a specific entity.
    pub(crate) fn visibility_state(&self, entity: Entity) -> Visibility {
        match &self.filter {
            VisibilityFilter::All => Visibility::Visible,
            VisibilityFilter::Blacklist { list, .. } => match list.get(&entity) {
                Some(BlacklistInfo::QueuedForRemoval) => Visibility::Gained,
                Some(BlacklistInfo::Hidden) => Visibility::Hidden,
                None => Visibility::Visible,
            },
            VisibilityFilter::Whitelist { list, .. } => match list.get(&entity) {
                Some(WhitelistInfo::JustAdded) => Visibility::Gained,
                Some(WhitelistInfo::Visible) => Visibility::Visible,
                None => Visibility::Hidden,
            },
        }
    }
}

/// Filter for [`ClientVisibility`] based on [`VisibilityPolicy`].
enum VisibilityFilter {
    All,
    Blacklist {
        /// All blacklisted entities and an indicator of whether they are in the queue for deletion
        /// at the end of this tick.
        list: EntityHashMap<BlacklistInfo>,

        /// All entities that were removed from the list in this tick.
        ///
        /// Visibility of these entities has been lost.
        added: EntityHashSet,

        /// All entities that were added to the list in this tick.
        ///
        /// Visibility of these entities has been gained.
        removed: EntityHashSet,
    },
    Whitelist {
        /// All whitelisted entities and an indicator of whether they were added to the list in
        /// this tick.
        list: EntityHashMap<WhitelistInfo>,

        /// All entities that were added to the list in this tick.
        ///
        /// Visibility of these entities has been gained.
        added: EntityHashSet,

        /// All entities that were removed from the list in this tick.
        ///
        /// Visibility of these entities has been lost.
        removed: EntityHashSet,
    },
}

#[derive(PartialEq, Clone, Copy)]
enum WhitelistInfo {
    Visible,
    JustAdded,
}

#[derive(PartialEq, Clone, Copy)]
enum BlacklistInfo {
    Hidden,
    QueuedForRemoval,
}

/// Visibility state for an entity in the current tick, from the perspective of one client.
///
/// Note that the distinction between 'lost visibility' and 'don't have visibility' is not exposed here.
/// There is only [`Visibility::Hidden`] to encompass both variants.
///
/// Lost visibility is handled separately with [`ClientVisibility::drain_lost_visibility`].
#[derive(PartialEq, Default, Clone, Copy)]
pub(crate) enum Visibility {
    /// The client does not have visibility of the entity in this tick.
    #[default]
    Hidden,
    /// The client gained visibility of the entity in this tick (it was not visible in the previous tick).
    Gained,
    /// The entity is visible to the client (and was visible in the previous tick).
    Visible,
}

enum VisibilityLostIter<T> {
    AllVisible,
    Lost(T),
}

impl<T: Iterator> Iterator for VisibilityLostIter<T> {
    type Item = T::Item;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            VisibilityLostIter::AllVisible => None,
            VisibilityLostIter::Lost(entities) => entities.next(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all() {
        let mut visibility = ClientVisibility::new(VisibilityPolicy::All);
        assert!(visibility.is_visible(Entity::PLACEHOLDER));

        visibility.set_visibility(Entity::PLACEHOLDER, true);
        assert!(visibility.is_visible(Entity::PLACEHOLDER));

        visibility.set_visibility(Entity::PLACEHOLDER, false);
        assert!(
            visibility.is_visible(Entity::PLACEHOLDER),
            "shouldn't have any effect for this policy"
        );
    }

    #[test]
    fn blacklist_insertion() {
        let mut visibility = ClientVisibility::new(VisibilityPolicy::Blacklist);
        visibility.set_visibility(Entity::PLACEHOLDER, false);
        assert!(!visibility.is_visible(Entity::PLACEHOLDER));

        let VisibilityFilter::Blacklist {
            list,
            added,
            removed,
        } = &visibility.filter
        else {
            panic!("filter should be a blacklist");
        };

        assert!(list.contains_key(&Entity::PLACEHOLDER));
        assert!(added.contains(&Entity::PLACEHOLDER));
        assert!(!removed.contains(&Entity::PLACEHOLDER));

        visibility.update();

        let VisibilityFilter::Blacklist {
            list,
            added,
            removed,
        } = &visibility.filter
        else {
            panic!("filter should be a blacklist");
        };

        assert!(list.contains_key(&Entity::PLACEHOLDER));
        assert!(!added.contains(&Entity::PLACEHOLDER));
        assert!(!removed.contains(&Entity::PLACEHOLDER));
    }

    #[test]
    fn blacklist_empty_removal() {
        let mut visibility = ClientVisibility::new(VisibilityPolicy::Blacklist);
        assert!(visibility.is_visible(Entity::PLACEHOLDER));

        visibility.set_visibility(Entity::PLACEHOLDER, true);
        assert!(visibility.is_visible(Entity::PLACEHOLDER));

        let VisibilityFilter::Blacklist {
            list,
            added,
            removed,
        } = visibility.filter
        else {
            panic!("filter should be a blacklist");
        };

        assert!(!list.contains_key(&Entity::PLACEHOLDER));
        assert!(!added.contains(&Entity::PLACEHOLDER));
        assert!(!removed.contains(&Entity::PLACEHOLDER));
    }

    #[test]
    fn blacklist_removal() {
        let mut visibility = ClientVisibility::new(VisibilityPolicy::Blacklist);
        visibility.set_visibility(Entity::PLACEHOLDER, false);
        visibility.update();
        visibility.set_visibility(Entity::PLACEHOLDER, true);
        assert!(visibility.is_visible(Entity::PLACEHOLDER));

        let VisibilityFilter::Blacklist {
            list,
            added,
            removed,
        } = &visibility.filter
        else {
            panic!("filter should be a blacklist");
        };

        assert!(list.contains_key(&Entity::PLACEHOLDER));
        assert!(!added.contains(&Entity::PLACEHOLDER));
        assert!(removed.contains(&Entity::PLACEHOLDER));

        visibility.update();

        let VisibilityFilter::Blacklist {
            list,
            added,
            removed,
        } = &visibility.filter
        else {
            panic!("filter should be a blacklist");
        };

        assert!(!list.contains_key(&Entity::PLACEHOLDER));
        assert!(!added.contains(&Entity::PLACEHOLDER));
        assert!(!removed.contains(&Entity::PLACEHOLDER));
    }

    #[test]
    fn blacklist_insertion_removal() {
        let mut visibility = ClientVisibility::new(VisibilityPolicy::Blacklist);

        // Insert and remove from the list.
        visibility.set_visibility(Entity::PLACEHOLDER, false);
        visibility.set_visibility(Entity::PLACEHOLDER, true);
        assert!(visibility.is_visible(Entity::PLACEHOLDER));

        let VisibilityFilter::Blacklist {
            list,
            added,
            removed,
        } = visibility.filter
        else {
            panic!("filter should be a blacklist");
        };

        assert!(!list.contains_key(&Entity::PLACEHOLDER));
        assert!(!added.contains(&Entity::PLACEHOLDER));
        assert!(!removed.contains(&Entity::PLACEHOLDER));
    }

    #[test]
    fn blacklist_duplicate_insertion() {
        let mut visibility = ClientVisibility::new(VisibilityPolicy::Blacklist);
        visibility.set_visibility(Entity::PLACEHOLDER, false);
        visibility.update();

        // Duplicate insertion.
        visibility.set_visibility(Entity::PLACEHOLDER, false);
        assert!(!visibility.is_visible(Entity::PLACEHOLDER));

        let VisibilityFilter::Blacklist {
            list,
            added,
            removed,
        } = visibility.filter
        else {
            panic!("filter should be a blacklist");
        };

        assert!(list.contains_key(&Entity::PLACEHOLDER));
        assert!(!added.contains(&Entity::PLACEHOLDER));
        assert!(!removed.contains(&Entity::PLACEHOLDER));
    }

    #[test]
    fn whitelist_insertion() {
        let mut visibility = ClientVisibility::new(VisibilityPolicy::Whitelist);
        visibility.set_visibility(Entity::PLACEHOLDER, true);
        assert!(visibility.is_visible(Entity::PLACEHOLDER));

        let VisibilityFilter::Whitelist {
            list,
            added,
            removed,
        } = &visibility.filter
        else {
            panic!("filter should be a whitelist");
        };

        assert!(list.contains_key(&Entity::PLACEHOLDER));
        assert!(added.contains(&Entity::PLACEHOLDER));
        assert!(!removed.contains(&Entity::PLACEHOLDER));

        visibility.update();

        let VisibilityFilter::Whitelist {
            list,
            added,
            removed,
        } = &visibility.filter
        else {
            panic!("filter should be a blacklist");
        };

        assert!(list.contains_key(&Entity::PLACEHOLDER));
        assert!(!added.contains(&Entity::PLACEHOLDER));
        assert!(!removed.contains(&Entity::PLACEHOLDER));
    }

    #[test]
    fn whitelist_empty_removal() {
        let mut visibility = ClientVisibility::new(VisibilityPolicy::Whitelist);
        assert!(!visibility.is_visible(Entity::PLACEHOLDER));

        visibility.set_visibility(Entity::PLACEHOLDER, false);
        assert!(!visibility.is_visible(Entity::PLACEHOLDER));

        let VisibilityFilter::Whitelist {
            list,
            added,
            removed,
        } = visibility.filter
        else {
            panic!("filter should be a whitelist");
        };

        assert!(!list.contains_key(&Entity::PLACEHOLDER));
        assert!(!added.contains(&Entity::PLACEHOLDER));
        assert!(!removed.contains(&Entity::PLACEHOLDER));
    }

    #[test]
    fn whitelist_removal() {
        let mut visibility = ClientVisibility::new(VisibilityPolicy::Whitelist);
        visibility.set_visibility(Entity::PLACEHOLDER, true);
        visibility.update();
        visibility.set_visibility(Entity::PLACEHOLDER, false);
        assert!(!visibility.is_visible(Entity::PLACEHOLDER));

        let VisibilityFilter::Whitelist {
            list,
            added,
            removed,
        } = &visibility.filter
        else {
            panic!("filter should be a whitelist");
        };

        assert!(!list.contains_key(&Entity::PLACEHOLDER));
        assert!(!added.contains(&Entity::PLACEHOLDER));
        assert!(removed.contains(&Entity::PLACEHOLDER));

        visibility.update();

        let VisibilityFilter::Whitelist {
            list,
            added,
            removed,
        } = &visibility.filter
        else {
            panic!("filter should be a blacklist");
        };

        assert!(!list.contains_key(&Entity::PLACEHOLDER));
        assert!(!added.contains(&Entity::PLACEHOLDER));
        assert!(!removed.contains(&Entity::PLACEHOLDER));
    }

    #[test]
    fn whitelist_insertion_removal() {
        let mut visibility = ClientVisibility::new(VisibilityPolicy::Whitelist);

        // Insert and remove from the list.
        visibility.set_visibility(Entity::PLACEHOLDER, true);
        visibility.set_visibility(Entity::PLACEHOLDER, false);
        assert!(!visibility.is_visible(Entity::PLACEHOLDER));

        let VisibilityFilter::Whitelist {
            list,
            added,
            removed,
        } = visibility.filter
        else {
            panic!("filter should be a blacklist");
        };

        assert!(!list.contains_key(&Entity::PLACEHOLDER));
        assert!(!added.contains(&Entity::PLACEHOLDER));
        assert!(!removed.contains(&Entity::PLACEHOLDER));
    }

    #[test]
    fn whitelist_duplicate_insertion() {
        let mut visibility = ClientVisibility::new(VisibilityPolicy::Whitelist);
        visibility.set_visibility(Entity::PLACEHOLDER, true);
        visibility.update();

        // Duplicate insertion.
        visibility.set_visibility(Entity::PLACEHOLDER, true);
        assert!(visibility.is_visible(Entity::PLACEHOLDER));

        let VisibilityFilter::Whitelist {
            list,
            added,
            removed,
        } = visibility.filter
        else {
            panic!("filter should be a blacklist");
        };

        assert!(list.contains_key(&Entity::PLACEHOLDER));
        assert!(!added.contains(&Entity::PLACEHOLDER));
        assert!(!removed.contains(&Entity::PLACEHOLDER));
    }
}
