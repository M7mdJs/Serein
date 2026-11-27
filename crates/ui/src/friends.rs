//! Friends overview using the retained relationship/presence state and existing user actions.
use crate::{
	MessagingUi, composer_text, design,
	icons::{self, Icon},
	profiles, user_menu,
};
use client_core::{Command, State};
use egui::{Color32, RichText, Stroke, vec2};

#[derive(Default)]
pub(super) struct Friends {
	pub(super) presence_warning_dismissed: Option<u64>,
	tab: Tab,
	query: String,
	username: String,
	outgoing: bool,
	list_key: Option<ListKey>,
	list_query: String,
	list: Box<[model::Id]>,
	#[cfg(feature = "demo")]
	focus_search: bool,
}
#[derive(PartialEq, Eq)]
struct ListKey {
	generation: u64,
	relationships: u64,
	online: Option<(bool, u64)>,
	tab: Tab,
}
#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum Tab {
	#[default]
	Online,
	All,
	Pending,
	Restricted,
	Add,
}
impl Friends {
	fn matches(&self, state: &State, user: &model::User, query: &str) -> bool {
		if self.tab == Tab::Online {
			let (status, _, _, _) = profiles::presence(state, user.id, None);
			if !matches!(status, Some("online" | "idle" | "dnd")) {
				return false;
			}
		}
		let username = if self.tab == Tab::Restricted {
			state
				.restricted_user(user.id)
				.map(|(_, name, _)| name.as_str())
		} else {
			state.friend_username(user.id)
		};
		query.is_empty()
			|| user.name.to_lowercase().contains(query)
			|| state.user_display_name(user).to_lowercase().contains(query)
			|| username.is_some_and(|name| name.to_lowercase().contains(query))
	}
	fn sync_list(&mut self, state: &State) -> bool {
		let key = ListKey {
			generation: state.generation,
			relationships: state.relationship_view(),
			online: (self.tab == Tab::Online)
				.then(|| (state.gateway_connected, state.direct_presence_epoch())),
			tab: self.tab,
		};
		if self.list_key.as_ref() == Some(&key) && self.list_query == self.query {
			return false;
		}
		let query = self.query.trim().to_lowercase();
		// ponytail: cold Online builds scan bounded presences; index only if rebuilds warrant it.
		let mut friends: Vec<_> = if self.tab == Tab::Restricted {
			state
				.restricted_users()
				.map(|(user, _, _)| user)
				.filter(|user| self.matches(state, user, &query))
				.collect()
		} else {
			state
				.friends()
				.filter(|user| self.matches(state, user, &query))
				.collect()
		};
		friends.sort_unstable_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
		// At most MAX_RELATIONSHIPS fixed-size IDs (32,000 bytes); no profiles retained.
		self.list = friends.into_iter().map(|user| user.id).collect();
		self.list_query = self.query.clone();
		self.list_key = Some(key);
		true
	}
}
impl MessagingUi {
	#[cfg(feature = "demo")]
	pub fn prepare_friends_sample(&mut self) {
		self.friends.tab = Tab::Online;
		self.friends.query.clear();
		self.friends.focus_search = true;
	}
	#[cfg(feature = "demo")]
	pub fn preview_friends_tab(&mut self, tab: &str) {
		self.friends.tab = match tab {
			"all" => Tab::All,
			"pending" => Tab::Pending,
			"blocked" => Tab::Restricted,
			"add" => Tab::Add,
			_ => Tab::Online,
		};
	}
	#[cfg(feature = "demo")]
	pub fn friends_sample_focused(&self, ctx: &egui::Context) -> bool {
		ctx.memory(|memory| memory.has_focus(egui::Id::unique("friends-search")))
	}
	fn add_friend_page(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		commands: &mut Vec<Command>,
	) {
		let colors = design::palette(ui);
		let language = self.language;
		egui::Frame::new().inner_margin(24).show(ui, |ui| {
			ui.label(
				design::semibold(ui, language.text("friends-add"), 20.0).color(colors.text_strong),
			);
			ui.add(
				egui::Label::new(
					RichText::new(language.text("friends-add-description"))
						.size(14.0)
						.color(colors.muted),
				)
				.wrap(),
			);
			ui.add_space(16.0);
			design::label(ui, &language.text("friends-username"));
			let busy = state.user_action_pending();
			let enabled = !busy
				&& !self.friends.username.trim().is_empty()
				&& (state.demo
					|| (state.gateway_connected
						&& state.auth == client_core::auth::AuthState::Authenticated));
			let label = if busy {
				language.text("friends-sending")
			} else {
				language.text("friends-send-request")
			};
			let mut send = false;
			let mut field = |ui: &mut egui::Ui, width: f32| {
				ui.allocate_ui(vec2(width, 40.0), |ui| {
					let input = design::bidi_input(
						ui,
						ui.scope_id().with("friends-add-username"),
						&mut self.friends.username,
						false,
						|edit| {
							edit.align(egui::Align2::LEFT_CENTER)
								.hint_text(language.text("friends-enter-username"))
								.char_limit(33)
						},
					);
					enabled
						&& input.lost_focus()
						&& ui.input(|input| input.key_pressed(egui::Key::Enter))
				})
				.inner
			};
			let action = |ui: &mut egui::Ui| {
				ui.add_enabled_ui(enabled, |ui| {
					design::button(ui, &label, design::ButtonKind::Primary)
				})
				.inner
				.clicked()
			};
			// The button sits beside the field when it fits and drops below it otherwise.
			let button = 170.0;
			if ui.available_width() >= 360.0 {
				ui.horizontal(|ui| {
					ui.spacing_mut().item_spacing.x = 8.0;
					send |= field(ui, ui.available_width() - button - 8.0);
					send |= action(ui);
				});
			} else {
				send |= field(ui, ui.available_width());
				ui.add_space(8.0);
				send |= action(ui);
			}
			if send && let Some(command) = state.add_friend(&self.friends.username) {
				commands.push(command);
			}
			if state.demo {
				design::hint(
					ui,
					&crate::i18n::translate(
						"friends-add-friend-page-offline-demo-actions-are-simulated",
					),
				);
			} else if !state.gateway_connected {
				design::hint(ui, &language.text("friends-reconnect"));
			}
			design::hint(ui, &language.text("friends-notes-unsupported"));
			design::divider(ui);
			ui.label(
				design::semibold(ui, language.text("friends-other-places"), 16.0)
					.color(colors.text_strong),
			);
			ui.add(
				egui::Label::new(
					RichText::new(language.text("friends-discover-description"))
						.size(14.0)
						.color(colors.muted),
				)
				.wrap(),
			);
			ui.add_space(8.0);
			ui.hyperlink_to(
				language.text("friends-explore-servers"),
				"https://discord.com/servers",
			);
		});
	}
	fn friend_requests_page(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		commands: &mut Vec<Command>,
	) {
		let colors = design::palette(ui);
		let language = self.language;
		let mut resolve = None;
		egui::Frame::new()
			.inner_margin(egui::Margin::symmetric(24, 16))
			.show(ui, |ui| {
				let [incoming, outgoing] = [false, true].map(|outgoing| {
					state
						.pending_friends()
						.filter(|(_, _, incoming)| *incoming != outgoing)
						.count()
				});
				let labels = [
					format!("{} — {incoming}", language.text("friends-incoming")),
					format!("{} — {outgoing}", language.text("friends-outgoing")),
				];
				if let Some(index) = design::segmented(
					ui,
					&[&labels[0], &labels[1]],
					usize::from(self.friends.outgoing),
				) {
					self.friends.outgoing = index == 1;
				}
				ui.add_space(12.0);
				search(
					ui,
					&mut self.friends.query,
					egui::Id::unique("friend-requests-search"),
					&language.text("friends-search-requests"),
				);
				ui.add_space(16.0);
				let query = self.friends.query.trim().to_lowercase();
				let mut rows: Vec<_> = state
					.pending_friends()
					.filter(|(user, name, incoming)| {
						*incoming != self.friends.outgoing
							&& (user.name.to_lowercase().contains(&query)
								|| name.to_lowercase().contains(&query))
					})
					.collect();
				rows.sort_unstable_by(|a, b| a.0.name.cmp(&b.0.name).then(a.0.id.cmp(&b.0.id)));
				if rows.is_empty() {
					let title = if !state.friend_requests_known() {
						language.text("friends-requests-unavailable")
					} else if !query.is_empty() {
						language.text("friends-no-request-search")
					} else if self.friends.outgoing {
						language.text("friends-no-outgoing")
					} else {
						language.text("friends-no-incoming")
					};
					let detail = if query.is_empty() {
						language.text("friends-new-requests")
					} else {
						language.text("friends-try-different")
					};
					design::empty_state(ui, Icon::People, &title, &detail);
					return;
				}
				section_label(
					ui,
					design::eyebrow(
						ui,
						format!(
							"{} — {}",
							if self.friends.outgoing {
								language.text("friends-outgoing")
							} else {
								language.text("friends-incoming")
							},
							rows.len()
						),
						colors.muted,
					),
				);
				ui.add_space(8.0);
				ui.spacing_mut().item_spacing.y = 0.0;
				let enabled =
					!state.user_action_pending() && (state.demo || state.gateway_connected);
				let mut after_hot = false;
				self.scroll
					.attach(
						ui,
						"friend-requests",
						egui::ScrollArea::vertical().auto_shrink([false, false]),
					)
					.show_rows(ui, ROW, rows.len(), |ui, range| {
						for (user, name, incoming) in &rows[range] {
							ui.push_id(user.id.0, |ui| {
								// Requests have no conversation to open, so the row opens the
								// profile like member lists; the round buttons stay on top.
								let (rect, response, hot) =
									person_row(ui, after_hot, egui::Sense::click());
								after_hot = hot;
								let response =
									response.on_hover_cursor(egui::CursorIcon::PointingHand);
								response.widget_info(|| {
									egui::WidgetInfo::labeled(egui::Role::Button, true, &user.name)
								});
								self.profile.person_click(ui, &response, None, user);
								user_menu::show(
									&response,
									state,
									user,
									user_menu::Place::Nowhere,
									&mut self.profile,
									&mut self.user_action,
								);
								let actions = if *incoming { 2 } else { 1 };
								let layout = RowLayout::new(rect, actions);
								self.avatars.show_plain(
									&mut ui
										.new_child(egui::UiBuilder::new().max_rect(layout.avatar)),
									user,
									40.0,
									state.demo,
								);
								let mut text =
									ui.new_child(egui::UiBuilder::new().max_rect(layout.text));
								text.spacing_mut().item_spacing.y = 1.0;
								text.add(
									egui::Label::new(
										design::semibold(ui, state.user_display_name(user), 15.0)
											.color(colors.text_strong),
									)
									.truncate(),
								);
								text.add(
									egui::Label::new(
										RichText::new(if *incoming {
											format!(
												"{name} · {}",
												language.text("friends-incoming-request")
											)
										} else {
											format!(
												"{name} · {}",
												language.text("friends-outgoing-request")
											)
										})
										.size(13.0)
										.color(colors.muted),
									)
									.truncate(),
								);
								let mut row = ui.new_child(
									egui::UiBuilder::new()
										.max_rect(layout.actions)
										.layout(egui::Layout::left_to_right(egui::Align::Center)),
								);
								row.spacing_mut().item_spacing.x = ACTION_GAP;
								row.add_enabled_ui(enabled, |ui| {
									ui.spacing_mut().item_spacing.x = ACTION_GAP;
									let reject_label = language.text(if *incoming {
										"friends-decline-request"
									} else {
										"friends-cancel-request"
									});
									if *incoming
										&& round_action(
											ui,
											Icon::Check,
											&language.text("friends-accept-request"),
											colors.positive,
										)
										.clicked()
									{
										resolve = Some((user.id, true));
									}
									if round_action(ui, Icon::Close, &reject_label, colors.danger)
										.clicked()
									{
										resolve = Some((user.id, false));
									}
								});
							});
						}
					});
			});
		if let Some((user, accept)) = resolve
			&& let Some(command) = state.resolve_friend_request(user, accept)
		{
			commands.push(command);
		}
	}
	/// Draws the Friends page: its tabs with the incoming request count on Pending, the selected
	/// list and its actions.
	pub(super) fn friends_page(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		commands: &mut Vec<Command>,
	) {
		let colors = design::palette(ui);
		let language = self.language;
		egui::Frame::new()
			.inner_margin(egui::Margin::symmetric(16, 8))
			.show(ui, |ui| {
				ui.horizontal_wrapped(|ui| {
					ui.set_min_height(32.0);
					ui.spacing_mut().item_spacing.x = 8.0;
					icons::inline(ui, Icon::People, 20.0, colors.muted);
					ui.label(
						design::semibold(ui, language.text("friends"), 16.0)
							.color(colors.text_strong),
					);
					let (rule, _) = ui.allocate_exact_size(vec2(17.0, 24.0), egui::Sense::hover());
					ui.painter().vline(
						rule.center().x,
						rule.y_range(),
						egui::Stroke::new(1.0, colors.border),
					);
					for (tab, title) in [
						(Tab::Online, language.text("friends-online")),
						(Tab::All, language.text("friends-all")),
						(Tab::Pending, language.text("friends-pending")),
						(Tab::Restricted, language.text("friends-blocked-ignored")),
						(Tab::Add, language.text("friends-add")),
					] {
						let count = (tab == Tab::Pending)
							.then(|| {
								state
									.pending_friends()
									.filter(|(_, _, incoming)| *incoming)
									.count()
							})
							.filter(|count| {
								*count > 0
									&& !crate::interface_tweaks::on(
										ui.ctx(),
										extensions::InterfaceTweak::HiddenRequestCounts,
									)
							});
						if header_tab(ui, &title, count, self.friends.tab == tab, tab == Tab::Add)
							.clicked() && self.friends.tab != tab
						{
							self.friends.tab = tab;
							self.friends.query.clear();
						}
					}
				});
			});
		ui.separator();
		if matches!(self.friends.tab, Tab::Online | Tab::All)
			&& state.gateway_connected
			&& state.startup_warnings.presence
			&& self.friends.presence_warning_dismissed != Some(state.generation)
		{
			egui::Frame::new()
				.inner_margin(egui::Margin::symmetric(24, 8))
				.show(ui, |ui| {
					ui.horizontal_top(|ui| {
						icons::inline(ui, Icon::ShieldWarning, 18.0, colors.warning);
						ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
							if icons::button(
								ui,
								Icon::Close,
								22.0,
								&language.text("friends-dismiss-warning"),
							)
							.clicked()
							{
								self.friends.presence_warning_dismissed = Some(state.generation);
							}
							ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
								ui.add(
									egui::Label::new(language.text("friends-status-warning"))
										.wrap(),
								);
							});
						});
					});
				});
		}
		let show_active_now = ui.available_width() >= 700.0;
		let (active_dm, active_join) = if show_active_now {
			let total = ui.available_width();
			let active_width = (total * 0.34).clamp(300.0, 360.0);
			let list_width = (total - active_width - 1.0).max(360.0);
			let height = ui.available_height();
			ui.horizontal_top(|ui| {
				ui.spacing_mut().item_spacing.x = 0.0;
				ui.allocate_ui_with_layout(
					vec2(list_width, height),
					egui::Layout::top_down(egui::Align::Min),
					|ui| {
						ui.set_min_width(list_width);
						self.friends_tab_body(ui, state, commands);
					},
				);
				let (rule, _) =
					ui.allocate_exact_size(vec2(1.0, height.max(1.0)), egui::Sense::hover());
				ui.painter().vline(
					rule.center().x,
					rule.y_range(),
					Stroke::new(1.0, colors.border),
				);
				ui.allocate_ui_with_layout(
					vec2(active_width, height),
					egui::Layout::top_down(egui::Align::Min),
					|ui| {
						ui.set_min_width(active_width);
						self.active_now_panel(ui, state)
					},
				)
				.inner
			})
			.inner
		} else {
			self.friends_tab_body(ui, state, commands);
			(None, None)
		};
		if let Some(user) = active_dm
			&& let Some(command) = state.open_friend_dm(user)
		{
			commands.push(command);
		}
		if let Some((guild, channel)) = active_join
			&& let Err(reason) = self.join_from_profile(state, guild, channel, commands)
		{
			self.toasts.push(design::Level::Error, reason);
		}
	}

	fn friends_tab_body(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		commands: &mut Vec<Command>,
	) {
		if self.friends.tab == Tab::Add {
			self.add_friend_page(ui, state, commands);
			return;
		}
		if self.friends.tab == Tab::Pending {
			self.friend_requests_page(ui, state, commands);
			return;
		}
		let colors = design::palette(ui);
		let language = self.language;
		let voice_users = profiles::voice_users(state);
		let mut selected = None;
		egui::Frame::new()
			.inner_margin(egui::Margin::symmetric(24, 16))
			.show(ui, |ui| {
				let search = search(
					ui,
					&mut self.friends.query,
					egui::Id::unique("friends-search"),
					&language.text("friends-search"),
				);
				#[cfg(feature = "demo")]
				if std::mem::take(&mut self.friends.focus_search) {
					search.request_focus();
				}
				#[cfg(not(feature = "demo"))]
				let _ = search;
				ui.add_space(20.0);
				self.friends.sync_list(state);
				section_label(
					ui,
					design::eyebrow(
						ui,
						format!(
							"{} \u{2014} {}",
							match self.friends.tab {
								Tab::All => language.text("friends-all-heading"),
								Tab::Restricted => language.text("friends-blocked-heading"),
								_ => language.text("friends-online"),
							},
							self.friends.list.len()
						),
						colors.muted,
					),
				);
				ui.add_space(8.0);
				ui.spacing_mut().item_spacing.y = 0.0;
				if self.friends.list.is_empty() {
					let searching = !self.friends.query.trim().is_empty();
					let title =
						if self.friends.tab == Tab::Restricted && !state.restricted_users_known() {
							language.text("friends-blocked-unavailable")
						} else if self.friends.tab != Tab::Restricted && !state.friends_known() {
							language.text("friends-unavailable")
						} else if searching {
							language.text(if self.friends.tab == Tab::Restricted {
								"friends-no-blocked-search"
							} else {
								"friends-no-search"
							})
						} else if self.friends.tab == Tab::Restricted {
							language.text("friends-no-blocked")
						} else if self.friends.tab == Tab::All {
							language.text("friends-none-yet")
						} else {
							language.text("friends-none-online")
						};
					let detail = if searching {
						language.text("friends-try-different")
					} else if self.friends.tab == Tab::Restricted {
						language.text("friends-blocked-help")
					} else {
						language.text("friends-add-help")
					};
					design::empty_state(
						ui,
						if self.friends.tab == Tab::Restricted {
							Icon::ShieldWarning
						} else {
							Icon::People
						},
						&title,
						&detail,
					);
					return;
				}
				let mut after_hot = false;
				self.scroll
					.attach(
						ui,
						if self.friends.tab == Tab::Restricted {
							"restricted-users"
						} else {
							"friends-list"
						},
						egui::ScrollArea::vertical().auto_shrink([false, false]),
					)
					.show_rows(ui, ROW, self.friends.list.len(), |ui, range| {
						for index in range {
							let restricted = (self.friends.tab == Tab::Restricted)
								.then(|| state.restricted_user(self.friends.list[index]))
								.flatten();
							let user = restricted
								.map(|(user, _, _)| user)
								.or_else(|| state.friend(self.friends.list[index]));
							let Some(user) = user else {
								continue;
							};
							ui.push_id(user.id.0, |ui| {
								let (status, custom, activities, clients) = if restricted.is_some()
								{
									(None, None, &[][..], model::ClientPlatforms::default())
								} else {
									let (status, custom, activities, clients) =
										profiles::presence(state, user.id, None);
									(
										profiles::shown_status(state, user.id, status),
										custom,
										activities,
										clients,
									)
								};
								let dm = restricted
									.is_none()
									.then(|| {
										state.channels.iter().find(|c| {
											c.guild.is_none()
												&& c.kind == 1 && c
												.recipients
												.iter()
												.any(|u| u.id == user.id)
										})
									})
									.flatten();
								let can_message = restricted.is_none()
									&& (dm.is_some()
										|| (!state.user_action_pending()
											&& (state.demo
												|| (state.gateway_connected
													&& state.auth == client_core::auth::AuthState::Authenticated))));
								let (rect, response, hot) =
									person_row(ui, after_hot, egui::Sense::click());
								after_hot = hot;
								let response = if can_message {
									response.on_hover_cursor(egui::CursorIcon::PointingHand)
								} else {
									response
								};
								response.widget_info(|| {
									egui::WidgetInfo::labeled(
										egui::Role::Button,
										can_message,
										&user.name,
									)
								});
								if can_message && response.clicked() {
									selected = Some(user.id);
								}
								user_menu::show(
									&response,
									state,
									user,
									user_menu::Place::Nowhere,
									&mut self.profile,
									&mut self.user_action,
								);
								let layout =
									RowLayout::new(rect, if restricted.is_none() { 2 } else { 1 });
								let avatar = self
									.avatars
									.show_plain(
										&mut ui.new_child(
											egui::UiBuilder::new().max_rect(layout.avatar),
										),
										user,
										40.0,
										state.demo,
									)
									.interact(egui::Sense::click())
									.on_hover_cursor(egui::CursorIcon::PointingHand);
								self.profile.person_click_full(ui, &avatar, user);
								user_menu::show(
									&avatar,
									state,
									user,
									user_menu::Place::Nowhere,
									&mut self.profile,
									&mut self.user_action,
								);
								if let Some(status) = status {
									profiles::presence_badge(
										ui,
										avatar.rect,
										status,
										clients,
										if hot { colors.hover } else { colors.chat },
									);
								}
								let mut text = ui.new_child(
									egui::UiBuilder::new()
										.max_rect(layout.text)
										.layout(egui::Layout::top_down(egui::Align::Min)),
								);
								text.spacing_mut().item_spacing.y = 1.0;
								text.spacing_mut().interact_size.y = 0.0;
								text.horizontal(|ui| {
									ui.spacing_mut().item_spacing.x = 6.0;
									let display = state.user_display_name(user);
									let username = restricted
										.map(|(_, name, _)| name.as_str())
										.or_else(|| state.friend_username(user.id));
									let reserved =
										if hot && username.is_some() { 96.0 } else { 0.0 };
									let max_name = (ui.available_width() - reserved).max(40.0);
									composer_text::singleline_label(
										ui,
										display,
										egui::FontId::new(15.0, design::semibold_family(ui.ctx())),
										colors.text_strong,
										max_name,
										egui::Sense::hover(),
									);
									if hot && let Some(username) = username {
										ui.add(
											egui::Label::new(
												RichText::new(username)
													.size(13.0)
													.color(colors.muted),
											)
											.truncate()
											.selectable(false),
										);
									}
								});
								let subtitle = restricted
									.map(|(_, _, ignored)| {
										language.text(if *ignored {
											"friends-ignored"
										} else {
											"friends-blocked"
										})
									})
									.or_else(|| profiles::subtitle(custom, activities))
									.unwrap_or_else(|| {
										status.map_or_else(
											|| language.text("friends-presence-unavailable"),
											|status| profiles::presence_label(status).to_owned(),
										)
									});
								text.horizontal(|ui| {
									ui.spacing_mut().item_spacing.x = 4.0;
									profiles::voice_badge(
										ui,
										restricted.is_none() && voice_users.contains(&user.id),
										true,
									);
									if let Some(activity) = activities.first() {
										icons::inline(
											ui,
											if activity.kind == 2 {
												Icon::Spotify
											} else {
												Icon::GameController
											},
											14.0,
											colors.positive,
										);
									}
									let sub_resp = composer_text::singleline_label(
										ui,
										&subtitle,
										egui::FontId::proportional(13.0),
										colors.muted,
										ui.available_width().max(24.0),
										egui::Sense::hover(),
									);
									sub_resp.on_hover_text(&subtitle);
								});
								let mut actions = ui.new_child(
									egui::UiBuilder::new()
										.max_rect(layout.actions)
										.layout(egui::Layout::left_to_right(egui::Align::Center)),
								);
								actions.spacing_mut().item_spacing.x = ACTION_GAP;
								if restricted.is_none() {
									let message = actions
										.add_enabled_ui(can_message, |ui| {
											round_action(
												ui,
												Icon::Threads,
												&language.text("friends-message"),
												colors.text_strong,
											)
										})
										.inner;
									if message.clicked() {
										selected = Some(user.id);
									}
								}
								let more = round_action(
									&mut actions,
									Icon::More,
									&language.text("friends-more"),
									colors.text_strong,
								);
								egui::Popup::menu(&more).show(|ui| {
									user_menu::contents(
										ui,
										state,
										user,
										user_menu::Place::Nowhere,
										&mut self.profile,
										&mut self.user_action,
										None,
									)
								});
							});
						}
					});
			});
		if let Some(user) = selected
			&& let Some(command) = state.open_friend_dm(user)
		{
			commands.push(command);
		}
	}

	/// Right-hand Discord-style Active Now panel displaying friends currently playing, streaming,
	/// listening, or in voice.
	fn active_now_panel(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
	) -> (Option<model::Id>, Option<(model::Id, model::Id)>) {
		let colors = design::palette(ui);
		let language = self.language;
		let active = active_friend_cards(state, self.calls_unavailable(state));
		let mut selected_dm = None;
		let mut join_voice = None;
		egui::Frame::new()
			.inner_margin(egui::Margin::symmetric(16, 16))
			.show(ui, |ui| {
				ui.label(
					design::semibold(ui, language.text("friends-active-now"), 18.0)
						.color(colors.text_strong),
				);
				ui.add_space(12.0);
				if active.is_empty() {
					egui::Frame::new()
						.fill(colors.raised)
						.stroke(Stroke::new(1.0, colors.border))
						.corner_radius(12)
						.inner_margin(18)
						.show(ui, |ui| {
							ui.set_width(ui.available_width());
							ui.vertical_centered(|ui| {
								ui.label(
									design::semibold(
										ui,
										language.text("friends-active-quiet"),
										15.0,
									)
									.color(colors.text_strong),
								);
								ui.add_space(6.0);
								ui.add(
									egui::Label::new(
										RichText::new(language.text("friends-active-quiet-detail"))
											.size(13.0)
											.color(colors.muted),
									)
									.wrap(),
								);
							});
						});
					return;
				}
				self.scroll
					.attach(
						ui,
						"friends-active-now",
						egui::ScrollArea::vertical().auto_shrink([false, false]),
					)
					.show(ui, |ui| {
						ui.spacing_mut().item_spacing.y = 10.0;
						for entry in active {
							let user = entry.user;
							let can_message = state.channels.iter().any(|c| {
								c.guild.is_none()
									&& c.kind == 1 && c.recipients.iter().any(|u| u.id == user.id)
							}) || (!state.user_action_pending()
								&& (state.demo
									|| (state.gateway_connected
										&& state.auth
											== client_core::auth::AuthState::Authenticated)));
							ui.push_id(("active-now", user.id.0), |ui| {
								let frame = egui::Frame::new()
									.fill(colors.raised)
									.stroke(Stroke::new(1.0, colors.border))
									.corner_radius(10)
									.inner_margin(10)
									.show(ui, |ui| {
										ui.set_width(ui.available_width());
										ui.spacing_mut().item_spacing.y = 6.0;
										ui.horizontal(|ui| {
											ui.spacing_mut().item_spacing.x = 8.0;
											let avatar = self
												.avatars
												.show_plain(ui, user, 32.0, state.demo)
												.interact(egui::Sense::click())
												.on_hover_cursor(egui::CursorIcon::PointingHand);
											if let Some(status) = entry.status {
												profiles::presence_badge(
													ui,
													avatar.rect,
													status,
													entry.clients,
													colors.raised,
												);
											}
											self.profile.person_click_full(ui, &avatar, user);
											user_menu::show(
												&avatar,
												state,
												user,
												user_menu::Place::Nowhere,
												&mut self.profile,
												&mut self.user_action,
											);
											ui.with_layout(
												egui::Layout::right_to_left(egui::Align::Center),
												|ui| {
													if let Some(icon) =
														entry.activities.first().and_then(|a| {
															a.image
																.as_ref()
																.or(a.small_image.as_ref())
														}) {
														let media = model::EmbedMedia {
															url: Some(icon.clone()),
															width: 24,
															height: 24,
															..Default::default()
														};
														self.avatars.show_media(
															ui,
															&media,
															egui::vec2(24.0, 24.0),
															state.demo,
															crate::avatars::Surface::Inline,
														);
													} else if can_message {
														let (btn_rect, btn_resp) = ui
															.allocate_exact_size(
																egui::Vec2::splat(26.0),
																egui::Sense::click(),
															);
														if btn_resp.hovered() {
															ui.painter().circle_filled(
																btn_rect.center(),
																13.0,
																colors.hover,
															);
														}
														icons::paint(
															ui.painter(),
															Icon::Threads,
															btn_rect.shrink(5.0),
															if btn_resp.hovered() {
																colors.text_strong
															} else {
																colors.muted
															},
														);
														if btn_resp
															.on_hover_text(
																language.text("friends-message"),
															)
															.clicked()
														{
															selected_dm = Some(user.id);
														}
													}
													ui.with_layout(
														egui::Layout::top_down(egui::Align::Min),
														|ui| {
															ui.spacing_mut().item_spacing.y = 0.0;
															let display =
																state.user_display_name(user);
															let max_w =
																ui.available_width().max(24.0);
															let name_resp =
																composer_text::singleline_label(
																	ui,
																	display,
																	egui::FontId::new(
																		14.0,
																		design::semibold_family(
																			ui.ctx(),
																		),
																	),
																	colors.text_strong,
																	max_w,
																	egui::Sense::click(),
																);
															self.profile.person_click_full(
																ui, &name_resp, user,
															);
															user_menu::show(
																&name_resp,
																state,
																user,
																user_menu::Place::Nowhere,
																&mut self.profile,
																&mut self.user_action,
															);
															let summary = entry
																.activities
																.first()
																.map(|a| a.name.clone())
																.or_else(|| {
																	entry.spot.as_ref().map(
																		|spot| {
																			format!(
																				"{} · {}",
																				spot.channel.name,
																				spot.guild.name
																			)
																		},
																	)
																})
																.unwrap_or_default();
															if !summary.is_empty() {
																let summary_resp =
																	composer_text::singleline_label(
																		ui,
																		&summary,
																		egui::FontId::proportional(
																			12.0,
																		),
																		colors.muted,
																		max_w,
																		egui::Sense::hover(),
																	);
																summary_resp
																	.on_hover_text(&summary);
															}
														},
													);
												},
											);
										});
										if !entry.activities.is_empty() || entry.spot.is_some() {
											egui::Frame::new()
												.fill(colors.chat)
												.corner_radius(8)
												.inner_margin(8)
												.show(ui, |ui| {
													ui.set_width(ui.available_width());
													ui.spacing_mut().item_spacing.y = 6.0;
													if let Some(activity) = entry.activities.first()
													{
														ui.horizontal(|ui| {
															ui.spacing_mut().item_spacing.x = 8.0;
															if let Some(icon) = activity
																.image
																.as_ref()
																.or(activity.small_image.as_ref())
															{
																let media = model::EmbedMedia {
																	url: Some(icon.clone()),
																	width: 30,
																	height: 30,
																	..Default::default()
																};
																self.avatars.show_media(
																	ui,
																	&media,
																	egui::vec2(30.0, 30.0),
																	state.demo,
																	crate::avatars::Surface::Inline,
																);
															}
															ui.vertical(|ui| {
																ui.spacing_mut().item_spacing.y =
																	1.0;
																let max_w =
																	ui.available_width().max(24.0);
																let primary = activity
																	.details
																	.as_deref()
																	.filter(|s| !s.is_empty())
																	.unwrap_or(&activity.name);
																composer_text::singleline_label(
																	ui,
																	primary,
																	egui::FontId::new(
																		13.0,
																		design::semibold_family(
																			ui.ctx(),
																		),
																	),
																	colors.text_strong,
																	max_w,
																	egui::Sense::hover(),
																);
																if let Some(detail) = activity
																	.state
																	.as_deref()
																	.filter(|s| !s.is_empty())
																{
																	composer_text::singleline_label(
																		ui,
																		detail,
																		egui::FontId::proportional(
																			12.0,
																		),
																		colors.muted,
																		max_w,
																		egui::Sense::hover(),
																	);
																}
															});
														});
													}
													if !entry.activities.is_empty()
														&& entry.spot.is_some()
													{
														design::card_divider(ui);
													}
													if let Some(spot) = &entry.spot {
														let can_join = spot.blocked.is_none();
														let row = ui.horizontal(|ui| {
															ui.spacing_mut().item_spacing.x = 8.0;
															let (icon_box, _) = ui
																.allocate_exact_size(
																	egui::Vec2::splat(26.0),
																	egui::Sense::hover(),
																);
															ui.painter().rect_filled(
																icon_box,
																6,
																colors.raised,
															);
															icons::paint(
																ui.painter(),
																Icon::Speaker,
																icon_box.shrink(5.0),
																colors.positive,
															);
															ui.vertical(|ui| {
																ui.spacing_mut().item_spacing.y =
																	0.0;
																let max_w =
																	ui.available_width().max(24.0);
																composer_text::singleline_label(
																	ui,
																	&spot.channel.name,
																	egui::FontId::new(
																		13.0,
																		design::semibold_family(
																			ui.ctx(),
																		),
																	),
																	colors.text_strong,
																	max_w,
																	egui::Sense::hover(),
																);
																composer_text::singleline_label(
																	ui,
																	&spot.guild.name,
																	egui::FontId::proportional(
																		11.5,
																	),
																	colors.muted,
																	max_w,
																	egui::Sense::hover(),
																);
															});
														});
														let click = ui.interact(
															row.response.rect,
															ui.id().with((
																"active-voice-join",
																spot.channel.id.0,
															)),
															if can_join {
																egui::Sense::click()
															} else {
																egui::Sense::hover()
															},
														);
														if can_join && click.hovered() {
															ui.painter().rect_stroke(
																row.response.rect.expand(2.0),
																6,
																Stroke::new(1.0, colors.border),
																egui::StrokeKind::Inside,
															);
														}
														if can_join
															&& click
																.on_hover_cursor(
																	egui::CursorIcon::PointingHand,
																)
																.clicked()
														{
															join_voice = Some((
																spot.guild.id,
																spot.channel.id,
															));
														}
													}
												});
										}
									});
								if frame.response.hovered() {
									ui.painter().rect_stroke(
										frame.response.rect,
										10,
										Stroke::new(
											1.0,
											design::mix(colors.border, colors.text, 0.18),
										),
										egui::StrokeKind::Inside,
									);
								}
							});
						}
					});
			});
		(selected_dm, join_voice)
	}
}

struct ActiveFriend<'a> {
	user: &'a model::User,
	status: Option<&'a str>,
	activities: &'a [model::RichActivity],
	clients: model::ClientPlatforms,
	spot: Option<profiles::VoiceSpot<'a>>,
}

/// Collects friends currently active (in a server voice channel or sharing a Rich Presence
/// activity), excluding blocked users and sorting voice/activity entries ahead of plain names.
fn active_friend_cards<'a>(
	state: &'a State,
	calls_blocked: Option<&'static str>,
) -> Vec<ActiveFriend<'a>> {
	let mut cards: Vec<_> = state
		.friends()
		.filter(|user| state.user_blocked(user.id) != Some(true))
		.filter_map(|user| {
			let (status, _, activities, clients) = profiles::presence(state, user.id, None);
			let status = profiles::shown_status(state, user.id, status);
			let spot = (!user.webhook && profiles::user_in_voice(state, user.id))
				.then(|| profiles::voice_spot(state, user.id, calls_blocked))
				.flatten();
			let online_activity =
				matches!(status, Some("online" | "idle" | "dnd")) && !activities.is_empty();
			(online_activity || spot.is_some()).then_some(ActiveFriend {
				user,
				status,
				activities,
				clients,
				spot,
			})
		})
		.collect();
	cards.sort_unstable_by(|a, b| {
		(
			a.spot.is_none(),
			a.activities.is_empty(),
			state.user_display_name(a.user).to_lowercase(),
			a.user.id,
		)
			.cmp(&(
				b.spot.is_none(),
				b.activities.is_empty(),
				state.user_display_name(b.user).to_lowercase(),
				b.user.id,
			))
	});
	cards.truncate(48);
	cards
}

/// List heading aligned with the row content inset.
fn section_label(ui: &mut egui::Ui, text: RichText) {
	ui.horizontal(|ui| {
		ui.spacing_mut().interact_size.y = 0.0;
		ui.add_space(10.0);
		ui.label(text);
	});
}

/// Row height shared by the friend, request and restricted lists.
const ROW: f32 = 62.0;
const ACTION: f32 = 36.0;
const ACTION_GAP: f32 = 10.0;

/// Avatar, text and right-aligned action slots inside one person row.
struct RowLayout {
	avatar: egui::Rect,
	text: egui::Rect,
	actions: egui::Rect,
}
impl RowLayout {
	fn new(rect: egui::Rect, actions: usize) -> Self {
		let inner = rect.shrink2(vec2(10.0, 0.0));
		let width = actions as f32 * ACTION + actions.saturating_sub(1) as f32 * ACTION_GAP;
		let actions = egui::Rect::from_min_max(
			egui::pos2(inner.right() - width, rect.center().y - ACTION / 2.0),
			egui::pos2(inner.right(), rect.center().y + ACTION / 2.0),
		);
		Self {
			avatar: egui::Rect::from_min_size(
				egui::pos2(inner.left(), rect.center().y - 20.0),
				vec2(40.0, 40.0),
			),
			text: egui::Rect::from_min_max(
				egui::pos2(inner.left() + 52.0, rect.center().y - 19.0),
				egui::pos2(
					(actions.left() - 12.0).max(inner.left() + 53.0),
					rect.center().y + 21.0,
				),
			),
			actions,
		}
	}
}

/// Full-width person row with a rounded hover fill and inset dividers that step aside for
/// the hovered row. Returns whether the row is hot so the next row can hide its divider.
fn person_row(
	ui: &mut egui::Ui,
	after_hot: bool,
	sense: egui::Sense,
) -> (egui::Rect, egui::Response, bool) {
	let colors = design::palette(ui);
	let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), sense);
	let hot = response.contains_pointer() || response.has_focus();
	if hot {
		ui.painter()
			.rect_filled(rect, 8, design::row_highlight(ui, colors.hover, 1.0));
	} else if !after_hot {
		ui.painter().hline(
			egui::Rangef::new(rect.left() + 10.0, rect.right() - 10.0),
			rect.top(),
			egui::Stroke::new(1.0, colors.border.gamma_multiply(0.65)),
		);
	}
	(rect, response, hot)
}

/// Round row action; `tint` colours the glyph while hovered.
fn round_action(ui: &mut egui::Ui, icon: Icon, label: &str, tint: Color32) -> egui::Response {
	let colors = design::palette(ui);
	let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(ACTION), egui::Sense::click());
	let enabled = ui.is_enabled();
	let hot = enabled && (response.hovered() || response.has_focus());
	let painter = ui.painter();
	painter.circle_filled(
		rect.center(),
		ACTION / 2.0,
		if hot {
			design::mix(colors.base, colors.text, 0.08)
		} else {
			colors.base
		},
	);
	if response.has_focus() {
		painter.circle_stroke(
			rect.center(),
			ACTION / 2.0 + 1.0,
			egui::Stroke::new(2.0, colors.accent),
		);
	}
	icons::paint(
		painter,
		icon,
		egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(18.0)),
		if !enabled {
			colors.muted.gamma_multiply(0.5)
		} else if hot {
			tint
		} else {
			colors.text
		},
	);
	response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
	response.on_hover_text(label)
}

/// Pill tab in the friends header; `accent` is the filled Add Friend call to action.
fn header_tab(
	ui: &mut egui::Ui,
	title: &str,
	count: Option<usize>,
	selected: bool,
	accent: bool,
) -> egui::Response {
	let colors = design::palette(ui);
	let font = egui::FontId::new(14.0, design::medium_family(ui.ctx()));
	let galley = ui
		.painter()
		.layout_no_wrap(title.to_owned(), font, Color32::WHITE);
	let badge = count.map(|count| {
		ui.painter().layout_no_wrap(
			count.min(99).to_string(),
			egui::FontId::new(11.0, design::semibold_family(ui.ctx())),
			Color32::WHITE,
		)
	});
	let badge_width = badge.as_ref().map_or(0.0, |b| b.size().x.max(8.0) + 14.0);
	let (rect, response) = ui.allocate_exact_size(
		vec2(galley.size().x + 20.0 + badge_width, 30.0),
		egui::Sense::click(),
	);
	let hot = response.hovered() || response.has_focus();
	let (fill, text) = if accent && selected {
		(colors.accent.gamma_multiply(0.18), colors.accent)
	} else if accent {
		(
			if hot {
				colors.accent.linear_multiply(1.1)
			} else {
				colors.accent
			},
			colors.accent_text,
		)
	} else if selected {
		(colors.selected, colors.text_strong)
	} else if hot {
		(colors.hover, colors.text)
	} else {
		(Color32::TRANSPARENT, colors.muted)
	};
	let painter = ui.painter();
	painter.rect_filled(rect, 6, fill);
	if response.has_focus() {
		painter.rect_stroke(
			rect.expand(1.0),
			7,
			egui::Stroke::new(2.0, colors.accent),
			egui::StrokeKind::Outside,
		);
	}
	let left = rect.left() + 10.0;
	painter.galley_with_override_text_color(
		egui::pos2(left, rect.center().y - galley.size().y / 2.0),
		galley.clone(),
		text,
	);
	if let Some(badge) = badge {
		let pill = egui::Rect::from_center_size(
			egui::pos2(
				left + galley.size().x + 6.0 + badge_width / 2.0 - 3.0,
				rect.center().y,
			),
			vec2(badge.size().x.max(8.0) + 8.0, 16.0),
		);
		painter.rect_filled(pill, 8, colors.badge_fill());
		painter.galley_with_override_text_color(
			pill.center() - badge.size() / 2.0,
			badge,
			Color32::WHITE,
		);
	}
	response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, title));
	response
}

/// Rounded search field with a trailing search glyph that becomes a clear button.
fn search(ui: &mut egui::Ui, query: &mut String, id: egui::Id, hint: &str) -> egui::Response {
	let colors = design::palette(ui);
	let frame = egui::Frame::new()
		.fill(colors.base)
		.corner_radius(8)
		.inner_margin(egui::Margin::symmetric(12, 6))
		.show(ui, |ui| {
			ui.horizontal(|ui| {
				let query_width = (ui.available_width() - 30.0).max(40.0);
				let edit = crate::composer_text::show_text_edit(ui, id, query, false, |edit| {
					edit.align(egui::Align2::LEFT_CENTER)
						.hint_text(hint)
						.char_limit(128)
						.frame(egui::Frame::NONE)
						.desired_width(query_width)
				})
				.response
				.response;
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					if query.is_empty() {
						icons::inline(ui, Icon::Search, 18.0, colors.muted);
					} else if icons::button(
						ui,
						Icon::Close,
						22.0,
						&crate::i18n::translate("friends-search-clear-search"),
					)
					.clicked()
					{
						query.clear();
					}
				});
				edit
			})
			.inner
		});
	let edit = frame.inner;
	ui.painter().rect_stroke(
		frame.response.rect,
		8,
		if edit.has_focus() {
			egui::Stroke::new(2.0, colors.accent)
		} else {
			egui::Stroke::new(1.0, colors.border)
		},
		egui::StrokeKind::Inside,
	);
	edit
}

#[cfg(test)]
mod tests {
	use super::*;
	use client_core::{Envelope, Event, user_actions::Event as Relationship};
	use model::{Id, Patch};

	fn apply(state: &mut State, event: Event) {
		state.apply(Envelope {
			generation: state.generation,
			event,
		});
	}

	// Keep the pre-cache selection algorithm as a behavioral oracle.
	fn uncached(friends: &Friends, state: &State) -> Vec<Id> {
		let query = friends.query.trim().to_lowercase();
		let filter = |user: &&model::User| {
			let (status, _, _, _) = profiles::presence(state, user.id, None);
			(friends.tab != Tab::Online || matches!(status, Some("online" | "idle" | "dnd")))
				&& (user.name.to_lowercase().contains(&query)
					|| state
						.user_display_name(user)
						.to_lowercase()
						.contains(&query)
					|| if friends.tab == Tab::Restricted {
						state
							.restricted_user(user.id)
							.map(|(_, name, _)| name.as_str())
					} else {
						state.friend_username(user.id)
					}
					.is_some_and(|name| name.to_lowercase().contains(&query)))
		};
		let mut rows: Vec<_> = if friends.tab == Tab::Restricted {
			state
				.restricted_users()
				.map(|(user, _, _)| user)
				.filter(filter)
				.collect()
		} else {
			state.friends().filter(filter).collect()
		};
		rows.sort_unstable_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
		rows.into_iter().map(|user| user.id).collect()
	}

	fn check_cache(friends: &mut Friends, state: &State) {
		friends.sync_list(state);
		assert_eq!(&*friends.list, uncached(friends, state));
		assert!(
			!friends.sync_list(state),
			"unchanged paint rebuilt the list"
		);
	}

	#[test]
	fn friends_cache_matches_uncached_across_relationship_and_session_changes() {
		let mut state = test_support::friends_demo_state();
		let mut friends = Friends::default();
		check_cache(&mut friends, &state);
		for _ in 0..100 {
			assert!(!friends.sync_list(&state));
		}
		for event in [Event::Disconnected, Event::Resumed] {
			apply(&mut state, event);
			assert!(friends.sync_list(&state));
			check_cache(&mut friends, &state);
		}
		for tab in [Tab::All, Tab::Online, Tab::Restricted] {
			friends.tab = tab;
			for query in ["", "robin.synthetic", " CASEY ", "no-match"] {
				friends.query = query.into();
				check_cache(&mut friends, &state);
			}
		}
		friends.tab = Tab::All;
		friends.query = "renamed".into();
		check_cache(&mut friends, &state);
		let mut user = state.friend(Id(1001)).unwrap().clone();
		user.name = "Renamed".into();
		for event in [
			Relationship::FriendProfile((user.clone(), "new.username".into())),
			Relationship::Nickname {
				user: user.id,
				text: "Other name".into(),
			},
			Relationship::Nicknames(vec![(user.id, "Renamed nickname".into())]),
			Relationship::Relationship {
				user: user.id,
				blocked: true,
			},
			Relationship::Relationship {
				user: user.id,
				blocked: false,
			},
			Relationship::Friend {
				user: user.id,
				friend: true,
				profile: Some((user.clone(), "new.username".into())),
			},
			Relationship::Friend {
				user: user.id,
				friend: false,
				profile: None,
			},
			Relationship::Friends(Some(vec![(user, "new.username".into())])),
			Relationship::Relationships(None),
			Relationship::Relationships(Some(vec![(Id(1001), false)])),
			Relationship::Friends(None),
		] {
			apply(&mut state, Event::UserAction(event));
			check_cache(&mut friends, &state);
		}
		state.logout();
		check_cache(&mut friends, &state);
		assert!(friends.list.is_empty());

		{
			let mut state = test_support::friends_demo_state();
			let mut friends = Friends::default();
			check_cache(&mut friends, &state);
			for status in [
				Patch::Absent,
				Patch::Value("idle".into()),
				Patch::Value("dnd".into()),
			] {
				apply(
					&mut state,
					Event::DirectPresence(vec![client_core::presence::Update {
						user: Id(1001),
						avatar: Patch::Absent,
						status,
						custom_status: Patch::Null,
						activities: Patch::Value(vec![model::RichActivity {
							kind: 0,
							name: "Synthetic game".into(),
							details: None,
							state: None,
							image: None,
							small_image: None,
							ends_at: None,
							started_at: None,
						}]),
						clients: Patch::Absent,
					}]),
				);
				assert!(!friends.sync_list(&state));
				let (_, custom, activities, _) = profiles::presence(&state, Id(1001), None);
				assert_eq!(
					profiles::subtitle(custom, activities).as_deref(),
					Some("Playing Synthetic game")
				);
				check_cache(&mut friends, &state);
			}
			for status in [
				Patch::Null,
				Patch::Value("online".into()),
				Patch::Value("offline".into()),
			] {
				apply(
					&mut state,
					Event::DirectPresence(vec![client_core::presence::Update {
						user: Id(1001),
						avatar: Patch::Absent,
						status,
						custom_status: Patch::Absent,
						activities: Patch::Absent,
						clients: Patch::Absent,
					}]),
				);
				assert!(friends.sync_list(&state));
				check_cache(&mut friends, &state);
			}
			friends.tab = Tab::All;
			check_cache(&mut friends, &state);
			apply(&mut state, Event::Disconnected);
			assert!(!friends.sync_list(&state));
			apply(&mut state, Event::Resumed);
			assert!(!friends.sync_list(&state));
			state.apply(Envelope {
				generation: state.generation + 1,
				event: Event::UserAction(Relationship::Friends(None)),
			});
			assert!(!friends.sync_list(&state));
			apply(&mut state, Event::Resync);
			check_cache(&mut friends, &state);
		}
	}

	#[test]
	fn friends_cache_replaces_ready_rows_and_rolls_back_optimistic_blocks() {
		for tab in [Tab::Online, Tab::All] {
			let mut state = test_support::friends_demo_state();
			let mut friends = Friends {
				tab,
				..Default::default()
			};
			check_cache(&mut friends, &state);
			let previous = friends.list.clone();
			let replacement: Vec<_> = state
				.friends()
				.map(|user| {
					let mut replacement = user.clone();
					replacement.id = Id(user.id.0 + 10_000);
					(
						replacement,
						state.friend_username(user.id).unwrap().to_owned(),
					)
				})
				.collect();
			let target = replacement[0].0.id;
			let presence = replacement
				.iter()
				.enumerate()
				.map(|(index, (user, _))| client_core::presence::Update {
					user: user.id,
					avatar: Patch::Absent,
					status: Patch::Value(if index < 7 { "online" } else { "offline" }.into()),
					custom_status: Patch::Null,
					activities: Patch::Null,
					clients: Patch::Absent,
				})
				.collect();
			let owner = state.user.clone().unwrap();
			let generation = state.generation;
			apply(
				&mut state,
				Event::Ready {
					permissions: Default::default(),
					user: owner,
					guilds: vec![],
					channels: vec![],
				},
			);
			apply(
				&mut state,
				Event::UserAction(Relationship::Relationships(Some(vec![]))),
			);
			apply(
				&mut state,
				Event::UserAction(Relationship::Friends(Some(replacement))),
			);
			apply(&mut state, Event::DirectPresence(presence));
			// Keep the old UI cache through READY and repopulation: equal row counts are not a key.
			assert_eq!(state.generation, generation);
			assert!(friends.sync_list(&state));
			assert_eq!(friends.list.len(), previous.len());
			assert!(friends.list.iter().all(|id| !previous.contains(id)));
			check_cache(&mut friends, &state);
			let restored = friends.list.clone();
			let revision = state.revision;
			let command = state.set_user_blocked(target, true).unwrap();
			assert_eq!(state.revision, revision);
			assert!(friends.sync_list(&state));
			assert_eq!(friends.list.len() + 1, restored.len());
			assert!(!friends.list.contains(&target));
			check_cache(&mut friends, &state);
			state.command_rejected(command);
			assert_eq!(state.revision, revision);
			assert!(friends.sync_list(&state));
			assert_eq!(friends.list, restored);
			check_cache(&mut friends, &state);
		}
	}

	#[test]
	fn active_now_collects_active_and_quiet_friends_and_renders_activity_cards() {
		let mut state = test_support::friends_demo_state();
		let friend_id = state.friends().next().expect("demo friend").id;
		apply(
			&mut state,
			Event::DirectPresence(vec![client_core::presence::Update {
				user: friend_id,
				avatar: Patch::Absent,
				status: Patch::Value("online".into()),
				custom_status: Patch::Null,
				activities: Patch::Value(vec![model::RichActivity {
					kind: 0,
					name: "Synthetic game".into(),
					details: Some("Ranked match".into()),
					state: None,
					image: None,
					small_image: None,
					ends_at: None,
					started_at: None,
				}]),
				clients: Patch::Absent,
			}]),
		);
		let cards = active_friend_cards(&state, None);
		assert!(cards.iter().any(|c| c.user.id == friend_id));

		let collect_text = |shape: &egui::Shape, out: &mut Vec<String>| {
			let mut stack = vec![shape];
			while let Some(next) = stack.pop() {
				match next {
					egui::Shape::Text(t) => out.push(t.galley.job.text.clone()),
					egui::Shape::Vec(v) => stack.extend(v.iter()),
					_ => {}
				}
			}
		};

		let ctx = egui::Context::default();
		let mut ui_view = MessagingUi::default();
		let mut commands = Vec::new();
		let active_out = ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(1000.0, 680.0),
				)),
				..Default::default()
			},
			|ui| ui_view.friends_page(ui, &mut state, &mut commands),
		);
		let mut active_texts = Vec::new();
		for clipped in &active_out.shapes {
			collect_text(&clipped.shape, &mut active_texts);
		}
		active_out.drop_without_applying_deltas();
		assert!(active_texts.iter().any(|t| t == "Active Now"));
		assert!(active_texts.iter().any(|t| t == "Synthetic game"));

		// Blocking an active friend removes them from Active Now immediately.
		apply(
			&mut state,
			Event::UserAction(Relationship::Relationship {
				user: friend_id,
				blocked: true,
			}),
		);
		assert!(
			active_friend_cards(&state, None)
				.iter()
				.all(|c| c.user.id != friend_id)
		);

		// Clearing presences renders the quiet Active Now empty card cleanly.
		state.apply(Envelope {
			generation: state.generation,
			event: Event::DirectPresence(
				state
					.friends()
					.map(|u| client_core::presence::Update {
						user: u.id,
						avatar: Patch::Absent,
						status: Patch::Value("online".into()),
						custom_status: Patch::Null,
						activities: Patch::Null,
						clients: Patch::Absent,
					})
					.collect(),
			),
		});
		assert!(active_friend_cards(&state, None).is_empty());
		let quiet_out = ctx.run_ui(
			egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(
					egui::Pos2::ZERO,
					egui::vec2(1000.0, 680.0),
				)),
				..Default::default()
			},
			|ui| ui_view.friends_page(ui, &mut state, &mut commands),
		);
		let mut quiet_texts = Vec::new();
		for clipped in &quiet_out.shapes {
			collect_text(&clipped.shape, &mut quiet_texts);
		}
		quiet_out.drop_without_applying_deltas();
		assert!(quiet_texts.iter().any(|t| t.contains("quiet")));
	}
}
