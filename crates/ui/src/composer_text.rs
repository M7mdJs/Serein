//! Inline artwork with the original wire text retained in egui's editing/undo model.
use crate::{avatars::Avatars, emoji};
use egui::{Color32, Image, text::CCursor, text::LayoutJob, text::TextFormat};
use model::User;
use std::{
	hash::{DefaultHasher, Hash, Hasher},
	ops::Range,
	sync::Arc,
};
use unicode_segmentation::UnicodeSegmentation;

struct Inline {
	source: Range<usize>,
	projected: usize,
	width: f32,
	label: Option<Arc<egui::Galley>>,
	background: Color32,
	image: Option<Image<'static>>,
	asset: Option<model::ImageShare>,
	icon: Option<crate::icons::Icon>,
	game_icon: Option<String>,
}

#[derive(Default)]
struct BidiCore {
	cache: Option<(u64, Arc<egui::Galley>)>,
	bidi: Option<Bidi>,
	/// The pointer gesture in progress over a right-to-left draft.
	bidi_gesture: Option<Gesture>,
	/// Preferred visual X (galley coordinates) across consecutive Up/Down steps.
	preferred_x: Option<f32>,
	/// Last interaction timestamp for caret blink phase without reading egui private fields.
	last_interaction_time: f64,
	/// Last observed cursor/selection range so caret moves pause cursor blinking.
	last_cursor: Option<egui::text::CCursorRange>,
}

#[derive(Default)]
pub(crate) struct Layout {
	inlines: Vec<Inline>,
	core: BidiCore,
	demo: bool,
}

/// A press selects from a char; a double-click selects whole words while the button is held.
#[derive(Clone)]
enum Gesture {
	Chars(usize),
	Words(Range<usize>),
}

/// The word (or space/punctuation run) holding char `index`, as a char range.
fn word_at(text: &str, index: usize) -> Range<usize> {
	let byte = text
		.char_indices()
		.nth(index)
		.map_or(text.len(), |(byte, _)| byte);
	let chars = |byte: usize| text[..byte].chars().count();
	text.split_word_bound_indices()
		.find(|(from, word)| (*from..from + word.len()).contains(&byte))
		.map_or(index..index, |(from, word)| {
			chars(from)..chars(from + word.len())
		})
}

/// The bidi paragraph holding byte `byte`; the end of the text belongs to the last one.
fn paragraph_at<'a>(
	bidi: &'a unicode_bidi::BidiInfo<'_>,
	byte: usize,
) -> Option<&'a unicode_bidi::ParagraphInfo> {
	bidi.paragraphs
		.iter()
		.find(|paragraph| paragraph.range.contains(&byte))
		.or_else(|| bidi.paragraphs.last().filter(|p| p.range.end == byte))
}

/// Visual placement of a draft with right-to-left text. egui's editor assumes glyphs run left
/// to right in logical order, so for these drafts the composer reorders each row, paints its
/// own caret and selection, and maps the pointer back to logical characters.
struct Bidi {
	/// Per source char: its visual cell within its row.
	cells: Vec<Cell>,
	rows: Vec<BidiRow>,
}

#[derive(Clone, Copy, Default)]
struct Cell {
	left: f32,
	width: f32,
	rtl: bool,
}

struct BidiRow {
	chars: Range<usize>,
	rect: egui::Rect,
	rtl: bool,
	ends_with_newline: bool,
}

#[derive(Clone, Copy)]
struct VisualStop {
	index: usize,
	x: f32,
	tie_x: f32,
	prefer_next_row: bool,
}

/// True when `text` has right-to-left letters, so the composer lays it out in visual order.
pub(crate) fn has_rtl(text: &str) -> bool {
	text.chars().any(|chr| {
		matches!(
			unicode_bidi::bidi_class(chr),
			unicode_bidi::BidiClass::R | unicode_bidi::BidiClass::AL
		)
	})
}

/// Whether the paragraph holding char `index` of `text` reads right to left.
#[cfg(test)]
pub(crate) fn paragraph_rtl(text: &str, index: usize) -> bool {
	let byte = text
		.char_indices()
		.nth(index)
		.map_or(text.len(), |(byte, _)| byte);
	let bidi = unicode_bidi::BidiInfo::new(text, None);
	paragraph_at(&bidi, byte).is_some_and(|paragraph| paragraph.level.is_rtl())
}

impl Layout {
	#[cfg(any(test, debug_assertions))]
	#[allow(clippy::too_many_arguments)]
	pub fn galley(
		&mut self,
		ui: &egui::Ui,
		text: &str,
		width: f32,
		users: &[User],
		roles: &[model::permissions::Role],
		channels: &[model::Channel],
		mass_mentions: bool,
		avatars: &mut Avatars,
		demo: bool,
	) -> Arc<egui::Galley> {
		self.galley_games(
			ui,
			text,
			width,
			users,
			roles,
			channels,
			&Default::default(),
			mass_mentions,
			avatars,
			demo,
		)
	}
	#[allow(clippy::too_many_arguments)]
	pub fn galley_games(
		&mut self,
		ui: &egui::Ui,
		text: &str,
		width: f32,
		users: &[User],
		roles: &[model::permissions::Role],
		channels: &[model::Channel],
		games: &client_core::game_mentions::Catalog,
		mass_mentions: bool,
		avatars: &mut Avatars,
		demo: bool,
	) -> Arc<egui::Galley> {
		self.demo = demo;
		let mut key = DefaultHasher::new();
		text.hash(&mut key);
		games.revision.hash(&mut key);
		width.to_bits().hash(&mut key);
		crate::fonts::revision(ui.ctx()).hash(&mut key);
		// An atlas reset moves every glyph, so a galley from before it samples the wrong pixels.
		ui.ctx()
			.fonts(|fonts| fonts.layout_generation())
			.hash(&mut key);
		ui.ctx().pixels_per_point().to_bits().hash(&mut key);
		egui::TextStyle::Body.resolve(ui.style()).hash(&mut key);
		let colors = crate::design::palette(ui);
		colors.text.hash(&mut key);
		colors.accent.hash(&mut key);
		colors.mention_text.hash(&mut key);
		colors.mention_bg.hash(&mut key);
		colors.muted.hash(&mut key);
		avatars.revision.hash(&mut key);
		emoji::ready(ui.ctx()).hash(&mut key);
		demo.hash(&mut key);
		if text.contains("<:") || text.contains("<a:") || text.contains("](https://") {
			static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
			(START
				.get_or_init(std::time::Instant::now)
				.elapsed()
				.as_secs() / 60)
				.hash(&mut key);
		}
		for user in users {
			user.id.hash(&mut key);
			user.name.hash(&mut key);
		}
		for role in roles {
			role.id.hash(&mut key);
			role.name.hash(&mut key);
		}
		for (start, _) in text.match_indices("<#") {
			if let Some((id, _)) = model::channel_mention_prefix(&text[start..]) {
				channels
					.iter()
					.find(|c| c.id == id)
					.map(|c| (&c.name, crate::channel_pill::icon(c, channels) as u8))
					.hash(&mut key);
			}
		}
		mass_mentions.hash(&mut key);
		let key = key.finish();
		if let Some((cached, galley)) = &self.core.cache
			&& *cached == key
		{
			return galley.clone();
		}
		self.inlines.clear();
		let font = egui::TextStyle::Body.resolve(ui.style());
		let colors = crate::design::palette(ui);
		let format = TextFormat::simple(font.clone(), colors.text);
		let size = emoji::inline_size(ui);
		let mut job = LayoutJob::default();
		job.wrap.max_width = width;
		if text.is_empty() {
			job.append("", 0.0, format.clone());
		}
		let mut byte = 0;
		let mut source = 0;
		let mut projected = 0;
		while byte < text.len() {
			let tail = &text[byte..];
			let mut label = None;
			let mut background = colors.accent.gamma_multiply(0.12);
			let mut image = None;
			let mut asset = None;
			let mut icon = None;
			let mut game_icon = None;
			let mut artwork = false;
			let length = if let Some((id, len)) = model::game_mention_prefix(tail) {
				label = Some((games.label(id), colors.mention_text));
				background = colors.mention_bg;
				icon = Some(crate::icons::Icon::GameController);
				game_icon = games.find(id).map(model::GameMention::icon_key);
				len
			} else if tail.starts_with("@game ")
				&& text[..byte]
					.chars()
					.next_back()
					.is_none_or(|c| c.is_whitespace() || matches!(c, '(' | '[' | '{'))
			{
				label = Some(("@game".into(), colors.text));
				background = colors.selected;
				5
			} else if let Some((id, len)) = model::user_mention_prefix(tail) {
				let name = users
					.iter()
					.find(|u| u.id == id)
					.map_or_else(|| id.to_string(), |user| user.name.clone());
				label = Some((format!("@{name}"), colors.mention_text));
				background = colors.mention_bg;
				len
			} else if let Some((id, len)) = model::role_mention_prefix(tail) {
				let name = roles
					.iter()
					.find(|role| role.id == id)
					.map_or_else(|| format!("unknown-role ({id})"), |role| role.name.clone());
				label = Some((format!("@{name}"), colors.mention_text));
				background = colors.mention_bg;
				len
			} else if let Some((id, len)) = model::channel_mention_prefix(tail) {
				let channel = channels.iter().find(|c| c.id == id && c.guild.is_some());
				let name =
					channel.map_or_else(|| format!("unknown-channel ({id})"), |c| c.name.clone());
				icon = Some(channel.map_or(crate::icons::Icon::Hash, |c| {
					crate::channel_pill::icon(c, channels)
				}));
				label = Some((name, colors.mention_text));
				background = colors.mention_bg;
				len
			} else if mass_mentions && let Some(len) = model::mass_mention_prefix(tail) {
				label = Some((tail[..len].to_owned(), colors.mention_text));
				background = colors.mention_bg;
				len
			} else if let Some((share, name, len)) = model::ImageShare::markdown_prefix(tail) {
				asset = Some(share);
				image = avatars.lookup_share_image(ui.ctx(), share, size, demo);
				if image.is_none() {
					label = Some((name, colors.muted));
				}
				len
			} else if let Some((id, len)) = emoji::custom_prefix(tail) {
				asset = Some(model::ImageShare::Emoji {
					id,
					animated: false,
				});
				image = avatars.lookup_share_image(ui.ctx(), asset.unwrap(), size, demo);
				// A useful name remains visible while artwork is unavailable/loading.
				if image.is_none() {
					let name = tail[..len]
						.trim_start_matches("<a:")
						.trim_start_matches("<:")
						.split(':')
						.next()
						.unwrap_or("emoji");
					label = Some((format!(":{name}:"), colors.muted));
				}
				len
			} else {
				let grapheme = tail.graphemes(true).next().expect("nonempty tail");
				image = emoji::image(ui.ctx(), grapheme, size);
				artwork = image.is_some() || emoji::lookup(grapheme).is_some();
				grapheme.len()
			};
			let label = label.map(|(text, color)| {
				let chip_max = (width - 6.0 - if icon.is_some() { size } else { 0.0 }).max(1.0);
				singleline_galley(ui, text, font.clone(), color, chip_max)
			});
			let raw = &tail[..length];
			let count = raw.chars().count();
			if label.is_some() || image.is_some() || artwork {
				let slot = label.as_ref().map_or(size, |g| {
					g.size().x + 6.0 + if icon.is_some() { size } else { 0.0 }
				});
				// One blank glyph forms an unbroken inline object, even at a row break.
				// Expand its character slots below, so native selection/copy/undo use wire text.
				job.append(" ", 0.0, emoji::inline_format(ui, slot, size));
				self.inlines.push(Inline {
					source: source..source + count,
					projected,
					width: slot,
					label,
					background,
					image,
					asset,
					icon,
					game_icon,
				});
				projected += 1;
			} else {
				job.append(raw, 0.0, format.clone());
				projected += count;
			}
			byte += length;
			source += count;
		}
		let mut galley = ui.fonts_mut(|f| f.layout_job(job));
		let reordered = has_rtl(text)
			.then(|| reorder(ui, &mut galley, width))
			.flatten();
		if self.inlines.is_empty() {
			self.core.bidi = reordered.map(|(cells, rtl)| Bidi::new(cells, &rtl, &galley, &[]));
			self.core.cache = Some((key, galley.clone()));
			return galley;
		}
		let galley_mut = Arc::make_mut(&mut galley);
		let chars: Vec<_> = text.chars().collect();
		let mut projected = 0;
		let mut next = 0;
		for placed in &mut galley_mut.rows {
			let row = Arc::make_mut(&mut placed.row);
			let mut glyphs = Vec::with_capacity(row.glyphs.len());
			for glyph in &row.glyphs {
				if let Some(inline) = self.inlines.get(next).filter(|i| i.projected == projected) {
					let left = reordered
						.as_ref()
						.and_then(|(cells, _)| cells.get(projected))
						.map_or(glyph.pos.x, |cell| cell.left);
					let count = inline.source.len();
					for (index, chr) in chars[inline.source.clone()].iter().enumerate() {
						let mut slot = *glyph;
						slot.chr = *chr;
						slot.pos.x = left + inline.width * index as f32 / count as f32;
						slot.advance_width = inline.width / count as f32;
						glyphs.push(slot);
					}
					next += 1;
				} else {
					glyphs.push(*glyph);
				}
				projected += 1;
			}
			row.glyphs = glyphs;
			projected += usize::from(placed.ends_with_newline);
		}
		// TextEdit's galley and TextBuffer must expose exactly the same original characters.
		galley_mut.job = Arc::new(LayoutJob::simple(text.to_owned(), font, colors.text, width));
		self.core.bidi =
			reordered.map(|(cells, rtl)| Bidi::new(cells, &rtl, &galley, &self.inlines));
		self.core.cache = Some((key, galley.clone()));
		galley
	}

	pub fn paint(
		&self,
		ui: &mut egui::Ui,
		output: &egui::text_edit::TextEditOutput,
		avatars: &mut Avatars,
	) {
		// TextEdit keeps the empty hint galley on the first keystroke.
		if self
			.core
			.cache
			.as_ref()
			.is_none_or(|(_, galley)| galley.job.text != output.galley.job.text)
		{
			return;
		}
		// The editor is taller than a scrolled composer; stay inside the scroll viewport too.
		let clip = output.text_clip_rect.intersect(ui.clip_rect());
		let painter = ui.painter().with_clip_rect(clip);
		for inline in &self.inlines {
			let rect = if let Some(bidi) = &self.core.bidi
				&& let Some(row) = bidi
					.rows
					.iter()
					.find(|row| row.chars.contains(&inline.source.start))
				&& let Some(cell) = bidi.cells.get(inline.source.start)
			{
				egui::Rect::from_min_size(
					egui::pos2(row.rect.left() + cell.left, row.rect.top()),
					egui::vec2(inline.width, row.rect.height()),
				)
				.translate(output.galley_pos.to_vec2())
			} else {
				let mut cursor = CCursor::new(inline.source.start);
				cursor.prefer_next_row = true;
				let position = output
					.galley
					.pos_from_cursor(cursor)
					.translate(output.galley_pos.to_vec2());
				egui::Rect::from_min_size(position.min, egui::vec2(inline.width, position.height()))
			};
			if !rect.intersect(clip).is_positive() {
				continue;
			}
			if let Some(asset) = inline.asset {
				// Only visible slots may request or refresh artwork, even on a cache hit.
				avatars.touch_share_image(ui.ctx(), asset, self.demo);
			}
			if let Some(label) = &inline.label {
				painter.rect_filled(rect, 3, inline.background);
				let offset = if let Some(icon) = inline.icon {
					let size = emoji::inline_size(ui);
					if let Some(key) = &inline.game_icon {
						let icon_rect = egui::Rect::from_center_size(
							egui::pos2(rect.left() + 3.0 + size / 2.0, rect.center().y),
							egui::Vec2::splat(size),
						);
						let mut child = ui.new_child(egui::UiBuilder::new().max_rect(icon_rect));
						child.set_clip_rect(clip);
						avatars.show_icon(&mut child, Some(key.clone()), size, self.demo, "Game");
					} else {
						crate::icons::paint(
							&painter,
							icon,
							egui::Rect::from_center_size(
								egui::pos2(rect.left() + 3.0 + size / 2.0, rect.center().y),
								egui::Vec2::splat(size * 0.65),
							),
							crate::design::palette(ui).mention_text,
						);
					}
					size
				} else {
					0.0
				};
				painter.galley(
					egui::pos2(
						rect.left() + 3.0 + offset,
						rect.center().y - label.size().y / 2.0,
					),
					label.clone(),
					Color32::PLACEHOLDER,
				);
			} else if let Some(image) = &inline.image {
				let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
				child.set_clip_rect(clip);
				image.paint_at(
					&child,
					egui::Rect::from_center_size(
						rect.center(),
						image.calc_size(egui::Vec2::splat(inline.width), image.size()),
					),
				);
			}
		}
	}

	/// Keep mouse/arrow selection endpoints outside encoded inline objects.
	pub fn snap_cursor(&self, output: &mut egui::text_edit::TextEditOutput, ctx: &egui::Context) {
		let Some(mut range) = output.cursor_range else {
			return;
		};
		let original = range;
		let direction = if self.core.bidi.is_some() {
			0
		} else {
			ctx.input(|i| {
				if i.key_pressed(egui::Key::ArrowLeft) {
					-1
				} else if i.key_pressed(egui::Key::ArrowRight) {
					1
				} else {
					0
				}
			})
		};
		for cursor in [&mut range.primary, &mut range.secondary] {
			for inline in &self.inlines {
				let index = cursor.index.0;
				if inline.source.start < index && index < inline.source.end {
					cursor.index = if direction < 0
						|| (direction == 0
							&& index - inline.source.start < inline.source.end - index)
					{
						inline.source.start.into()
					} else {
						inline.source.end.into()
					};
					break;
				}
			}
		}
		if range != original {
			output.cursor_range = Some(range);
			output.state.cursor.set_char_range(Some(range));
			output.state.clone().store(ctx, output.response.id);
		}
	}

	/// Let the native editor delete a selected token, preserving its normal undo history.
	pub fn select_deleted_inline(&self, ctx: &egui::Context, id: egui::Id) {
		if !ctx.memory(|m| m.has_focus(id)) {
			return;
		}
		let Some(mut state) = egui::text_edit::TextEditState::load(ctx, id) else {
			return;
		};
		let Some(range) = state.cursor.char_range().filter(|r| r.is_empty()) else {
			return;
		};
		let (back, forward) = ctx.input(|i| {
			(
				i.key_pressed(egui::Key::Backspace),
				i.key_pressed(egui::Key::Delete),
			)
		});
		if let Some(inline) = self.inlines.iter().find(|i| {
			(back && i.source.end == range.primary.index.0)
				|| (forward && i.source.start == range.primary.index.0)
		}) {
			state
				.cursor
				.set_char_range(Some(egui::text::CCursorRange::two(
					CCursor::new(inline.source.start),
					CCursor::new(inline.source.end),
				)));
			state.store(ctx, id);
		}
	}
}

/// Lay out a draft holding right-to-left text in visual order. Lines break in logical order
/// (wrapping at spaces by measured cluster advances), then Unicode bidi rules L1/L2 apply per
/// row and each row is shaped once as a directional line. egui's own rows cannot be reordered:
/// it emits one glyph per shaped glyph, so an Arabic letter drawn as a base and a dot becomes two.
/// Each row here has one glyph per char, in logical order at its visual position, and rows of
/// right-to-left paragraphs are right-aligned. Returns each projected char's cell and each
/// row's paragraph direction.
fn reorder(
	ui: &egui::Ui,
	galley: &mut Arc<egui::Galley>,
	width: f32,
) -> Option<(Vec<Cell>, Vec<bool>)> {
	use egui::epaint::text::PlacedRow;
	let job = galley.job.clone();
	let text = job.text.as_str();
	let bidi = unicode_bidi::BidiInfo::new(text, None);
	if !bidi.has_rtl() {
		return None;
	}
	let offsets: Vec<usize> = text
		.char_indices()
		.map(|(byte, _)| byte)
		.chain([text.len()])
		.collect();
	let char_at = |byte: usize| offsets.partition_point(|offset| *offset < byte);
	let mut cells = vec![Cell::default(); offsets.len() - 1];
	let mut directions = Vec::new();
	let mut rows = Vec::new();
	let mut y = 0.0;
	let mut fallback_glyph = galley
		.rows
		.iter()
		.flat_map(|row| row.row.glyphs.iter())
		.next()
		.copied();
	let template_for = |ui: &egui::Ui,
	                    fallback: &mut Option<egui::epaint::text::Glyph>,
	                    row: &egui::epaint::text::Row| {
		if let Some(first) = row.glyphs.first().copied() {
			*fallback = Some(first);
			return first;
		}
		*fallback.get_or_insert_with(|| {
			let format = job
				.sections
				.first()
				.map(|section| section.format.clone())
				.unwrap_or_default();
			let mut space = LayoutJob::default();
			space.append(" ", 0.0, format);
			ui.fonts_mut(|fonts| fonts.layout_job(space)).rows[0]
				.row
				.glyphs[0]
		})
	};
	let mut place = |row: egui::epaint::text::Row, rtl: bool, ends_with_newline: bool| {
		let x = if rtl && width.is_finite() {
			(width - row.size.x).max(0.0)
		} else {
			0.0
		};
		let height = row.size.y;
		rows.push(PlacedRow {
			pos: egui::pos2(x, y),
			row: Arc::new(row),
			ends_with_newline,
		});
		directions.push(rtl);
		y += height;
	};
	let mut hard_start = 0;
	for hard in text.split('\n') {
		let line = hard_start..hard_start + hard.len();
		hard_start = line.end + 1;
		let ends_with_newline = line.end < text.len();
		let cr = usize::from(hard.ends_with('\r'));
		let content_end = line.end - cr;
		let paragraph = paragraph_at(&bidi, line.start)?;
		let rtl = paragraph.level.is_rtl();
		if line.start >= content_end {
			let (mut row, _) = shape_line(ui, &job, &bidi, line.start..line.start);
			if cr == 1 {
				let cr_char = char_at(content_end);
				cells[cr_char] = Cell {
					left: 0.0,
					width: 0.0,
					rtl,
				};
				let mut glyph = template_for(ui, &mut fallback_glyph, &row);
				glyph.chr = '\r';
				glyph.pos.x = 0.0;
				glyph.advance_width = 0.0;
				glyph.first_vertex = row.visuals.glyph_vertex_range.end as u32;
				row.glyphs = vec![glyph];
			}
			place(row, rtl, ends_with_newline);
			continue;
		}
		// Advances of the whole hard line choose the logical breaks; rows are shaped again.
		let (_, measured) = shape_line(ui, &job, &bidi, line.start..content_end);
		let mut advances: Vec<(Range<usize>, f32)> = measured
			.into_iter()
			.map(|(range, rect, _)| (range, rect.width()))
			.collect();
		advances.sort_by_key(|(range, _)| range.start);
		let mut start = line.start;
		while start < content_end {
			let mut end = start;
			let mut used = 0.0;
			let mut word = None;
			for (range, advance) in advances.iter().filter(|(range, _)| range.start >= start) {
				if used + advance > width && end > start {
					break;
				}
				used += advance;
				end = range.end;
				if text[start..end].ends_with(char::is_whitespace) {
					word = Some(end);
				}
			}
			if end < content_end
				&& let Some(word) = word
			{
				end = word;
			}
			if end <= start {
				// A cluster/grapheme wider than the input still gets its own row without splitting marks.
				end = text[start..content_end]
					.graphemes(true)
					.next()
					.map_or(content_end, |g| start + g.len());
			}
			let (mut row, clusters) = shape_line(ui, &job, &bidi, start..end);
			for (range, rect, cluster_rtl) in clusters {
				let chars = char_at(range.start)..char_at(range.end);
				let share = rect.width() / chars.len().max(1) as f32;
				for (index, char) in chars.enumerate() {
					let left = if cluster_rtl {
						rect.max.x - share * (index + 1) as f32
					} else {
						rect.min.x + share * index as f32
					};
					cells[char] = Cell {
						left,
						width: share,
						rtl: cluster_rtl,
					};
				}
			}
			let at_line_end = end >= content_end;
			let row_end = if at_line_end { line.end } else { end };
			if at_line_end && cr == 1 {
				let cr_char = char_at(content_end);
				let cr_left = if rtl { 0.0 } else { row.size.x };
				cells[cr_char] = Cell {
					left: cr_left,
					width: 0.0,
					rtl,
				};
			}
			let template = template_for(ui, &mut fallback_glyph, &row);
			let glyph_end = row.visuals.glyph_vertex_range.end as u32;
			row.glyphs = text[start..row_end]
				.chars()
				.zip(&cells[char_at(start)..char_at(row_end)])
				.map(|(chr, cell)| {
					let mut glyph = template;
					glyph.chr = chr;
					// Caret X inside the cell so egui's scroll/IME follow the true visual caret.
					glyph.pos.x = if cell.rtl {
						cell.left + cell.width
					} else {
						cell.left
					};
					glyph.advance_width = cell.width;
					// egui recolours selected glyphs by vertex order; the composer
					// paints the selection itself.
					glyph.first_vertex = glyph_end;
					glyph
				})
				.collect();
			start = end;
			place(row, rtl, ends_with_newline && start >= content_end);
		}
	}
	let galley_mut = Arc::make_mut(galley);
	let mut rect = rows.iter().fold(egui::Rect::NOTHING, |rect, placed| {
		rect.union(egui::Rect::from_min_size(placed.pos, placed.row.size))
	});
	// Right-aligned RTL rows start at `placed.pos.x > 0`; keep `rect.min.x = 0.0` so
	// `TextEdit` (which subtracts `galley.rect.left()`) and `paint_bidi` (`galley_pos`) agree.
	if rect.is_finite() && rect.min.x > 0.0 {
		rect.min.x = 0.0;
	}
	galley_mut.rect = rect;
	galley_mut.mesh_bounds = rows.iter().fold(egui::Rect::NOTHING, |rect, placed| {
		rect.union(
			placed
				.row
				.visuals
				.mesh_bounds
				.translate(placed.pos.to_vec2()),
		)
	});
	galley_mut.num_vertices = rows.iter().map(|p| p.row.visuals.mesh.vertices.len()).sum();
	galley_mut.num_indices = rows.iter().map(|p| p.row.visuals.mesh.indices.len()).sum();
	galley_mut.elided = false;
	galley_mut.rows = rows;
	Some((cells, directions))
}

/// Shape `line` (job bytes) as one visual-order line. Splits on paragraph boundaries safely so
/// pasted separators like U+2029 or stray `\r` never panic or leak tofu glyphs.
fn shape_line(
	ui: &egui::Ui,
	job: &LayoutJob,
	bidi: &unicode_bidi::BidiInfo<'_>,
	line: Range<usize>,
) -> (
	egui::epaint::text::Row,
	Vec<(Range<usize>, egui::Rect, bool)>,
) {
	use egui::text::{ByteIndex, LayoutSection};
	let text = job.text.as_str();
	let mut shaped = LayoutJob::default();
	let mut pieces = Vec::new();
	let mut directions = Vec::new();
	let default_rtl = paragraph_at(bidi, line.start).is_some_and(|p| p.level.is_rtl());
	if line.is_empty() {
		// An empty row still takes the height of the text around it.
		let format = job
			.sections
			.first()
			.map(|section| section.format.clone())
			.unwrap_or_default();
		shaped.sections.push(LayoutSection {
			leading_space: 0.0,
			byte_range: ByteIndex(0)..ByteIndex(0),
			format,
		});
		directions.push(default_rtl);
	} else {
		let mut pos = line.start;
		while pos < line.end {
			let Some(paragraph) = paragraph_at(bidi, pos) else {
				break;
			};
			let sub_end = line.end.min(paragraph.range.end).max(pos);
			let (levels, runs) = bidi.visual_runs(paragraph, pos..sub_end);
			for run in runs {
				let rtl = levels[run.start].is_rtl();
				let mut parts: Vec<_> = job
					.sections
					.iter()
					.filter_map(|section| {
						let from = run.start.max(section.byte_range.start.0);
						let to = run.end.min(section.byte_range.end.0);
						(from < to).then_some((from..to, &section.format))
					})
					.flat_map(|(range, format)| {
						crate::rtl::script_parts(text, range)
							.into_iter()
							.map(move |range| (range, format))
					})
					.collect();
				if rtl {
					parts.reverse();
				}
				for (range, format) in parts {
					// Strip any bare internal `\r` while keeping source byte slices intact.
					let mut slice_start = range.start;
					for (rel, _) in text[range.clone()].match_indices('\r') {
						let cr_at = range.start + rel;
						if cr_at > slice_start {
							let piece_range = slice_start..cr_at;
							let from = shaped.text.len();
							shaped.text.push_str(&text[piece_range.clone()]);
							shaped.sections.push(LayoutSection {
								leading_space: 0.0,
								byte_range: ByteIndex(from)..ByteIndex(shaped.text.len()),
								format: format.clone(),
							});
							directions.push(rtl);
							pieces.push((piece_range, from, rtl));
						}
						slice_start = cr_at + 1;
					}
					if slice_start < range.end {
						let piece_range = slice_start..range.end;
						let from = shaped.text.len();
						shaped.text.push_str(&text[piece_range.clone()]);
						// append() merges equal formats and would erase a bidi boundary.
						shaped.sections.push(LayoutSection {
							leading_space: 0.0,
							byte_range: ByteIndex(from)..ByteIndex(shaped.text.len()),
							format: format.clone(),
						});
						directions.push(rtl);
						pieces.push((piece_range, from, rtl));
					}
				}
			}
			if sub_end >= line.end {
				break;
			}
			pos = sub_end;
		}
		if shaped.sections.is_empty() {
			let format = job
				.sections
				.first()
				.map(|section| section.format.clone())
				.unwrap_or_default();
			shaped.sections.push(LayoutSection {
				leading_space: 0.0,
				byte_range: ByteIndex(0)..ByteIndex(0),
				format,
			});
			directions.push(default_rtl);
		}
	}
	let native = ui.fonts_mut(|fonts| fonts.layout_single_line_with_clusters(shaped, &directions));
	let placed = &native.galley.rows[0];
	let origin = placed.pos.to_vec2();
	let clusters = native
		.clusters
		.iter()
		.filter_map(|cluster| {
			let (range, from, rtl) = pieces.iter().find(|(range, from, _)| {
				(*from..from + range.len()).contains(&cluster.bytes.start)
			})?;
			let bytes =
				range.start + cluster.bytes.start - from..range.start + cluster.bytes.end - from;
			Some((bytes, cluster.rect.translate(-origin), *rtl))
		})
		.collect();
	((*placed.row).clone(), clusters)
}

impl Bidi {
	/// Expand projected cells to source chars: an inline object's wire text shares its slot.
	fn new(projected: Vec<Cell>, rtl: &[bool], galley: &egui::Galley, inlines: &[Inline]) -> Self {
		let mut cells = Vec::with_capacity(projected.len());
		let mut next = inlines.iter().peekable();
		for (index, cell) in projected.into_iter().enumerate() {
			match next.peek() {
				Some(inline) if inline.projected == index => {
					cells.extend(std::iter::repeat_n(cell, inline.source.len()));
					next.next();
				}
				_ => cells.push(cell),
			}
		}
		let mut rows = Vec::with_capacity(galley.rows.len());
		let mut start = 0;
		for (placed, rtl) in galley.rows.iter().zip(rtl) {
			let end = start + placed.row.glyphs.len();
			rows.push(BidiRow {
				chars: start..end,
				rect: egui::Rect::from_min_size(placed.pos, placed.row.size),
				rtl: *rtl,
				ends_with_newline: placed.ends_with_newline,
			});
			start = end + usize::from(placed.ends_with_newline);
		}
		Self { cells, rows }
	}

	fn row_index_for_cursor(&self, cursor: CCursor) -> Option<usize> {
		let index = cursor.index.0;
		let mut soft_prev = None;
		for (ri, row) in self.rows.iter().enumerate() {
			if row.chars.start <= index && index < row.chars.end {
				if let Some(prev_ri) = soft_prev
					&& !cursor.prefer_next_row
				{
					return Some(prev_ri);
				}
				return Some(ri);
			}
			if row.chars.end == index {
				if !row.ends_with_newline
					&& self
						.rows
						.get(ri + 1)
						.is_some_and(|next| next.chars.start == index)
				{
					soft_prev = Some(ri);
				} else {
					return Some(ri);
				}
			}
		}
		soft_prev.or_else(|| (!self.rows.is_empty()).then_some(self.rows.len() - 1))
	}

	fn caret_in_row(&self, ri: usize, index: usize) -> Option<(egui::Rect, f32)> {
		let row = self.rows.get(ri)?;
		let (local_x, _) = self.caret_local_in_row(row, index);
		Some((row.rect, row.rect.left() + local_x))
	}

	fn caret_local_in_row(&self, row: &BidiRow, index: usize) -> (f32, f32) {
		if index < row.chars.end
			&& let Some(cell) = self.cells.get(index)
		{
			let x = if cell.rtl {
				cell.left + cell.width
			} else {
				cell.left
			};
			let tie_x = cell.left + cell.width * 0.5;
			return (x, tie_x);
		}
		if index > row.chars.start
			&& let Some(cell) = self.cells.get(index - 1)
		{
			let x = if cell.rtl {
				cell.left
			} else {
				cell.left + cell.width
			};
			let tie_x = cell.left + cell.width * 0.5;
			return (x, tie_x);
		}
		let x = if row.rtl { row.rect.width() } else { 0.0 };
		(x, x)
	}

	/// The caret's row rect and X for logical position `index`, on the side of the char it touches.
	#[cfg(test)]
	fn caret(&self, index: usize) -> Option<(egui::Rect, f32)> {
		self.caret_cursor(CCursor::new(index))
	}

	/// Soft-wrap aware caret geometry respecting `CCursor::prefer_next_row`.
	fn caret_cursor(&self, cursor: CCursor) -> Option<(egui::Rect, f32)> {
		let ri = self.row_index_for_cursor(cursor)?;
		self.caret_in_row(ri, cursor.index.0)
	}

	/// Visual left-to-right stops on `row`, keeping inline chips atomic and breaking dual-caret ties
	/// using each boundary's adjacent cell midpoint.
	fn visual_stops(&self, ri: usize, inlines: &[Inline]) -> Vec<VisualStop> {
		let row = &self.rows[ri];
		let mut stops = Vec::with_capacity(row.chars.len() + 1);
		for idx in row.chars.start..=row.chars.end {
			if inlines
				.iter()
				.any(|inline| inline.source.start < idx && idx < inline.source.end)
			{
				continue;
			}
			// Do not stop between trailing `\r` and `\n`; keep the caret before `\r`.
			if idx == row.chars.end
				&& row.chars.end > row.chars.start
				&& self.cells.get(idx - 1).is_some_and(|c| c.width == 0.0)
			{
				continue;
			}
			let (local_x, tie_local) = self.caret_local_in_row(row, idx);
			let prefer_next_row = idx == row.chars.start;
			stops.push(VisualStop {
				index: idx,
				x: row.rect.left() + local_x,
				tie_x: row.rect.left() + tie_local,
				prefer_next_row,
			});
		}
		if stops.is_empty() {
			let x = row.rect.left() + if row.rtl { row.rect.width() } else { 0.0 };
			stops.push(VisualStop {
				index: row.chars.start,
				x,
				tie_x: x,
				prefer_next_row: true,
			});
		}
		stops.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.tie_x.total_cmp(&b.tie_x)));
		stops
	}

	fn snap_out_of_inline(&self, mut index: usize, forward: bool, inlines: &[Inline]) -> usize {
		for inline in inlines {
			if inline.source.start < index && index < inline.source.end {
				index = if forward {
					inline.source.end
				} else {
					inline.source.start
				};
				break;
			}
		}
		index
	}

	fn local_rtl(&self, ri: usize, index: usize) -> bool {
		let row = &self.rows[ri];
		if index < row.chars.end
			&& let Some(cell) = self.cells.get(index)
		{
			return cell.rtl;
		}
		if index > row.chars.start
			&& let Some(cell) = self.cells.get(index - 1)
		{
			return cell.rtl;
		}
		row.rtl
	}

	fn index_at_with_bias(&self, pos: egui::Pos2) -> (usize, bool) {
		let Some((ri, row)) = self.rows.iter().enumerate().min_by(|(_, a), (_, b)| {
			let distance =
				|row: &BidiRow| (pos.y - pos.y.clamp(row.rect.top(), row.rect.bottom())).abs();
			distance(a).total_cmp(&distance(b))
		}) else {
			return (0, false);
		};
		let _ = ri;
		let x = pos.x - row.rect.left();
		let Some((index, cell)) = row
			.chars
			.clone()
			.filter(|&i| {
				self.cells
					.get(i)
					.is_some_and(|cell| cell.width > 0.0 || i == row.chars.start)
			})
			.map(|i| (i, self.cells[i]))
			.min_by(|a, b| {
				let distance = |cell: &Cell| (x - x.clamp(cell.left, cell.left + cell.width)).abs();
				distance(&a.1).total_cmp(&distance(&b.1))
			})
		else {
			return (row.chars.start, true);
		};
		let before = (x < cell.left + cell.width / 2.0) != cell.rtl;
		let target = if before { index } else { index + 1 };
		(target, target == row.chars.start)
	}

	/// The logical position nearest `pos` (galley coordinates), on the visual side clicked.
	#[cfg(test)]
	fn index_at(&self, pos: egui::Pos2) -> usize {
		self.index_at_with_bias(pos).0
	}

	fn step_horizontal(
		&self,
		text: &str,
		cursor: CCursor,
		right: bool,
		word: bool,
		inlines: &[Inline],
	) -> CCursor {
		if self.rows.is_empty() {
			return cursor;
		}
		let ri = self.row_index_for_cursor(cursor).unwrap_or(0);
		if word {
			let rtl = self.local_rtl(ri, cursor.index.0);
			let logical_forward = right == rtl;
			let mut next = if logical_forward {
				next_word_char(text, cursor.index.0)
			} else {
				prev_word_char(text, cursor.index.0)
			};
			next = self.snap_out_of_inline(next, logical_forward, inlines);
			return CCursor {
				index: next.into(),
				prefer_next_row: logical_forward
					&& self.rows.get(ri).is_some_and(|r| next > r.chars.end),
			};
		}
		let stops = self.visual_stops(ri, inlines);
		let pos = stops
			.iter()
			.position(|s| s.index == cursor.index.0)
			.unwrap_or_else(|| {
				let (_, target_x) = self.caret_local_in_row(&self.rows[ri], cursor.index.0);
				let target_x = self.rows[ri].rect.left() + target_x;
				stops
					.iter()
					.enumerate()
					.min_by(|(_, a), (_, b)| {
						(a.x - target_x).abs().total_cmp(&(b.x - target_x).abs())
					})
					.map_or(0, |(idx, _)| idx)
			});
		if !right {
			if pos > 0 {
				let s = stops[pos - 1];
				return CCursor {
					index: s.index.into(),
					prefer_next_row: s.prefer_next_row,
				};
			}
			// Off the visual left edge of the current row.
			if self.rows[ri].rtl {
				if let Some(next_ri) = (ri + 1 < self.rows.len()).then_some(ri + 1) {
					let next_stops = self.visual_stops(next_ri, inlines);
					let s = *next_stops.last().unwrap();
					return CCursor {
						index: s.index.into(),
						prefer_next_row: true,
					};
				}
			} else if let Some(prev_ri) = ri.checked_sub(1) {
				let prev_stops = self.visual_stops(prev_ri, inlines);
				let s = *prev_stops.last().unwrap();
				return CCursor {
					index: s.index.into(),
					prefer_next_row: false,
				};
			}
		} else {
			if pos + 1 < stops.len() {
				let s = stops[pos + 1];
				return CCursor {
					index: s.index.into(),
					prefer_next_row: s.prefer_next_row,
				};
			}
			// Off the visual right edge of the current row.
			if self.rows[ri].rtl {
				if let Some(prev_ri) = ri.checked_sub(1) {
					let prev_stops = self.visual_stops(prev_ri, inlines);
					let s = *prev_stops.first().unwrap();
					return CCursor {
						index: s.index.into(),
						prefer_next_row: false,
					};
				}
			} else if let Some(next_ri) = (ri + 1 < self.rows.len()).then_some(ri + 1) {
				let next_stops = self.visual_stops(next_ri, inlines);
				let s = *next_stops.first().unwrap();
				return CCursor {
					index: s.index.into(),
					prefer_next_row: true,
				};
			}
		}
		cursor
	}

	fn step_vertical(
		&self,
		cursor: CCursor,
		down: bool,
		preferred_x: f32,
		inlines: &[Inline],
	) -> CCursor {
		if self.rows.is_empty() {
			return cursor;
		}
		let ri = self.row_index_for_cursor(cursor).unwrap_or(0);
		let target_ri = if down {
			if ri + 1 >= self.rows.len() {
				let last = self.rows.last().unwrap();
				return CCursor {
					index: last.chars.end.into(),
					prefer_next_row: false,
				};
			}
			ri + 1
		} else {
			if ri == 0 {
				return CCursor {
					index: self.rows[0].chars.start.into(),
					prefer_next_row: true,
				};
			}
			ri - 1
		};
		let stops = self.visual_stops(target_ri, inlines);
		let chosen = stops
			.iter()
			.min_by(|a, b| {
				(a.x - preferred_x)
					.abs()
					.total_cmp(&(b.x - preferred_x).abs())
			})
			.unwrap();
		CCursor {
			index: chosen.index.into(),
			prefer_next_row: chosen.prefer_next_row,
		}
	}

	fn handle_navigation(
		&self,
		preferred_x: &mut Option<f32>,
		ctx: &egui::Context,
		id: egui::Id,
		text: &str,
		ime_active: bool,
		inlines: &[Inline],
	) -> bool {
		if ime_active || !ctx.memory(|m| m.has_focus(id)) || self.rows.is_empty() {
			return false;
		}
		let Some(mut state) = egui::text_edit::TextEditState::load(ctx, id) else {
			return false;
		};
		let Some(mut range) = state.cursor.char_range() else {
			return false;
		};
		let os = ctx.os();
		let mut changed = false;
		let mut pending = Vec::new();
		ctx.input_mut(|input| {
			input.events.retain(|event| {
				let egui::Event::Key {
					key,
					modifiers,
					pressed: true,
					..
				} = event
				else {
					if matches!(event, egui::Event::Text(_) | egui::Event::Paste(_)) {
						*preferred_x = None;
					}
					return true;
				};
				if matches!(
					key,
					egui::Key::ArrowLeft
						| egui::Key::ArrowRight
						| egui::Key::ArrowUp
						| egui::Key::ArrowDown
						| egui::Key::Home | egui::Key::End
				) {
					pending.push((*key, *modifiers));
					false
				} else {
					*preferred_x = None;
					true
				}
			});
		});
		if pending.is_empty() {
			return false;
		}
		let total_chars = text.chars().count();
		for (key, modifiers) in pending {
			let mac = os == egui::os::OperatingSystem::Mac;
			let word = if mac {
				modifiers.alt && !modifiers.command
			} else {
				modifiers.ctrl || modifiers.command
			};
			let doc_jump = if mac {
				modifiers.command
			} else {
				modifiers.ctrl
			};
			match key {
				egui::Key::ArrowLeft | egui::Key::ArrowRight => {
					*preferred_x = None;
					let right = key == egui::Key::ArrowRight;
					if mac && modifiers.command {
						let ri = self.row_index_for_cursor(range.primary).unwrap_or(0);
						let stops = self.visual_stops(ri, inlines);
						let s = if right {
							*stops.last().unwrap()
						} else {
							*stops.first().unwrap()
						};
						range.primary = CCursor {
							index: s.index.into(),
							prefer_next_row: s.prefer_next_row,
						};
						if !modifiers.shift {
							range.secondary = range.primary;
						}
						changed = true;
						continue;
					}
					if !modifiers.shift
						&& !modifiers.ctrl && !modifiers.alt
						&& !modifiers.command
						&& !range.is_empty()
					{
						// Collapse selection to the visual left/right end on screen.
						let [min, max] = range.sorted_cursors();
						let pick = match (self.caret_cursor(min), self.caret_cursor(max)) {
							(Some((r_min, x_min)), Some((r_max, x_max)))
								if (r_min.center().y - r_max.center().y).abs() < 1.0 =>
							{
								if right == (x_min <= x_max) {
									max
								} else {
									min
								}
							}
							_ => {
								let ri = self.row_index_for_cursor(min).unwrap_or(0);
								let rtl = self.rows.get(ri).is_some_and(|r| r.rtl);
								if right == rtl { min } else { max }
							}
						};
						range = egui::text::CCursorRange::one(pick);
						changed = true;
						continue;
					}
					let next = self.step_horizontal(text, range.primary, right, word, inlines);
					range.primary = next;
					if !modifiers.shift {
						range.secondary = range.primary;
					}
					changed = true;
				}
				egui::Key::ArrowUp | egui::Key::ArrowDown => {
					let down = key == egui::Key::ArrowDown;
					if doc_jump && mac {
						*preferred_x = None;
						range.primary = if down {
							CCursor::new(total_chars)
						} else {
							CCursor::new(0)
						};
					} else {
						let x = *preferred_x.get_or_insert_with(|| {
							self.caret_cursor(range.primary).map_or(0.0, |(_, x)| x)
						});
						range.primary = self.step_vertical(range.primary, down, x, inlines);
					}
					if !modifiers.shift {
						range.secondary = range.primary;
					}
					changed = true;
				}
				egui::Key::Home | egui::Key::End => {
					*preferred_x = None;
					let end = key == egui::Key::End;
					if doc_jump {
						range.primary = CCursor {
							index: if end { total_chars } else { 0 }.into(),
							prefer_next_row: !end,
						};
					} else {
						let ri = self.row_index_for_cursor(range.primary).unwrap_or(0);
						let row = &self.rows[ri];
						let target = if end {
							let cr_adjust = usize::from(
								row.chars.end > row.chars.start
									&& self
										.cells
										.get(row.chars.end - 1)
										.is_some_and(|c| c.width == 0.0),
							);
							row.chars.end - cr_adjust
						} else {
							row.chars.start
						};
						range.primary = CCursor {
							index: target.into(),
							prefer_next_row: !end,
						};
					}
					if !modifiers.shift {
						range.secondary = range.primary;
					}
					changed = true;
				}
				_ => {}
			}
		}
		if changed {
			range.h_pos = None;
			state.cursor.set_char_range(Some(range));
			state.store(ctx, id);
		}
		changed
	}
}

fn next_word_char(text: &str, index: usize) -> usize {
	let mut skipped = 0usize;
	for (word_idx, word) in text.split_word_bounds().enumerate() {
		let _ = word_idx;
		let count = word.chars().count();
		let end = skipped + count;
		if end > index && !word.chars().all(char::is_whitespace) {
			return end;
		}
		skipped = end;
	}
	text.chars().count()
}

fn prev_word_char(text: &str, index: usize) -> usize {
	let mut pos = 0usize;
	let mut candidate = 0usize;
	for word in text.split_word_bounds() {
		let count = word.chars().count();
		if pos >= index {
			break;
		}
		if !word.chars().all(char::is_whitespace) {
			candidate = pos;
		}
		pos += count;
	}
	candidate
}

impl BidiCore {
	/// Whether `text` is laid out in visual order, so the editor draws its caret and selection.
	fn bidi_active(&self, text: &str) -> bool {
		self.bidi.is_some()
			&& self
				.cache
				.as_ref()
				.is_some_and(|(_, galley)| galley.job.text == text)
	}

	fn prepare_navigation(
		&mut self,
		ctx: &egui::Context,
		id: egui::Id,
		text: &str,
		ime_active: bool,
		inlines: &[Inline],
	) {
		if !self.bidi_active(text) {
			self.preferred_x = None;
			return;
		}
		if let Some(bidi) = &self.bidi
			&& bidi.handle_navigation(&mut self.preferred_x, ctx, id, text, ime_active, inlines)
		{
			self.last_interaction_time = ctx.input(|i| i.time);
		}
	}

	/// Map the pointer to logical characters for a right-to-left draft. egui hit-tests its
	/// galley assuming glyphs grow left to right, which picks the wrong char once rows reorder.
	fn bidi_pointer(&mut self, output: &mut egui::text_edit::TextEditOutput, ctx: &egui::Context) {
		let Some(bidi) = &self.bidi else {
			self.bidi_gesture = None;
			return;
		};
		let response = &output.response;
		let Some(pointer) = response
			.interact_pointer_pos()
			.filter(|_| ctx.input(|i| i.pointer.primary_down() || i.pointer.primary_released()))
		else {
			self.bidi_gesture = None;
			return;
		};
		self.preferred_x = None;
		let (index, prefer_next_row) =
			bidi.index_at_with_bias((pointer - output.galley_pos).to_pos2());
		let text = output.galley.job.text.as_str();
		let (pressed, shift) = ctx.input(|i| (i.pointer.primary_pressed(), i.modifiers.shift));
		if response.triple_clicked() {
			let chars: Vec<char> = text.chars().collect();
			let start = chars[..index.min(chars.len())]
				.iter()
				.rposition(|c| *c == '\n')
				.map_or(0, |i| i + 1);
			let end = chars[index.min(chars.len())..]
				.iter()
				.position(|c| *c == '\n')
				.map_or(chars.len(), |i| index + i);
			self.bidi_gesture = Some(Gesture::Words(start..end));
		} else if response.double_clicked() {
			self.bidi_gesture = Some(Gesture::Words(word_at(text, index)));
		} else if pressed || self.bidi_gesture.is_none() {
			self.bidi_gesture = Some(Gesture::Chars(match output.state.cursor.char_range() {
				Some(range) if shift => range.secondary.index.0,
				_ => index,
			}));
		}
		let range = match self.bidi_gesture.clone() {
			Some(Gesture::Words(words)) => {
				// Dragging after a double-click extends by whole words in either direction.
				let under = word_at(text, index.min(text.chars().count().saturating_sub(1)));
				if under.start < words.start {
					(words.end, under.start)
				} else if under.end > words.end {
					(words.start, under.end)
				} else {
					(words.start, words.end)
				}
			}
			Some(Gesture::Chars(anchor)) => (anchor, index),
			None => (index, index),
		};
		let mut primary = CCursor::new(range.1);
		primary.prefer_next_row = prefer_next_row && range.0 == range.1;
		let range = egui::text::CCursorRange {
			primary,
			secondary: CCursor::new(range.0),
			h_pos: None,
		};
		if pressed || output.cursor_range != Some(range) {
			self.last_interaction_time = ctx.input(|i| i.time);
		}
		if output.cursor_range != Some(range) {
			output.cursor_range = Some(range);
			output.state.cursor.set_char_range(Some(range));
			output.state.clone().store(ctx, output.response.id);
		}
	}

	fn finish_paint(
		&mut self,
		ui: &mut egui::Ui,
		output: &mut egui::text_edit::TextEditOutput,
		slot: egui::layers::ShapeIdx,
		hid_native: bool,
		outer_visuals: &egui::Visuals,
	) {
		if output.response.changed() || self.last_cursor != output.cursor_range {
			self.last_interaction_time = ui.input(|i| i.time);
			self.last_cursor = output.cursor_range;
		}
		if let Some(bidi) = &self.bidi {
			self.paint_bidi_internal(
				ui,
				bidi,
				output,
				slot,
				outer_visuals,
				self.last_interaction_time,
			);
		} else if hid_native {
			paint_ltr_fallback(ui, output, slot, outer_visuals, self.last_interaction_time);
		}
	}

	fn paint_bidi_internal(
		&self,
		ui: &mut egui::Ui,
		bidi: &Bidi,
		output: &mut egui::text_edit::TextEditOutput,
		slot: egui::layers::ShapeIdx,
		visuals: &egui::Visuals,
		last_interaction_time: f64,
	) {
		if !output.response.has_focus() {
			return;
		}
		let Some(range) = output.cursor_range else {
			return;
		};
		let origin = output.galley_pos.to_vec2();
		let clip = output.text_clip_rect.intersect(ui.clip_rect());
		let [min, max] = range.sorted_cursors();
		let selection: Vec<egui::Shape> = (min.index.0..max.index.0)
			.filter_map(|index| {
				let row = bidi.rows.iter().find(|row| row.chars.contains(&index))?;
				let cell = bidi.cells.get(index)?;
				if cell.width <= 0.0 {
					return None;
				}
				let rect = egui::Rect::from_min_size(
					egui::pos2(row.rect.left() + cell.left, row.rect.top()),
					egui::vec2(cell.width, row.rect.height()),
				)
				.translate(origin)
				.intersect(clip);
				rect.is_positive()
					.then(|| egui::Shape::rect_filled(rect, 0.0, visuals.selection.bg_fill))
			})
			.collect();
		ui.painter().set(slot, selection);
		let Some((row, x)) = bidi.caret_cursor(range.primary) else {
			return;
		};
		let top = egui::pos2(x, row.top()) + origin;
		let bottom = egui::pos2(x, row.bottom()) + origin;
		let caret_rect = egui::Rect::from_min_max(top, bottom);
		if ui.memory(|mem| mem.owns_ime_events(output.response.id)) {
			let to_global = ui
				.ctx()
				.layer_transform_to_global(ui.layer_id())
				.unwrap_or_default();
			ui.output_mut(|o| {
				if let Some(ime) = &mut o.ime {
					ime.cursor_rect = to_global * caret_rect;
				}
			});
		}
		if ui.input(|i| i.focused) && (clip.contains(top) || clip.contains(bottom)) {
			let painter = ui.painter().with_clip_rect(clip);
			paint_blinking_caret(ui, &painter, visuals, top, bottom, last_interaction_time);
		}
	}
}

fn paint_blinking_caret(
	ui: &egui::Ui,
	painter: &egui::Painter,
	visuals: &egui::Visuals,
	top: egui::Pos2,
	bottom: egui::Pos2,
	last_interaction_time: f64,
) {
	if visuals.text_cursor.blink {
		let now = ui.input(|i| i.time);
		let on = visuals.text_cursor.on_duration;
		let off = visuals.text_cursor.off_duration;
		let total = (on + off).max(0.05);
		let elapsed = ((now - last_interaction_time).max(0.0) % f64::from(total)) as f32;
		let wake_secs = if elapsed < on {
			painter.line_segment([top, bottom], visuals.text_cursor.stroke);
			(on - elapsed).max(0.01)
		} else {
			(total - elapsed).max(0.01)
		};
		ui.ctx()
			.request_repaint_after(std::time::Duration::from_secs_f32(wake_secs));
	} else {
		painter.line_segment([top, bottom], visuals.text_cursor.stroke);
	}
}

fn paint_ltr_fallback(
	ui: &egui::Ui,
	output: &egui::text_edit::TextEditOutput,
	slot: egui::layers::ShapeIdx,
	visuals: &egui::Visuals,
	last_interaction_time: f64,
) {
	if !output.response.has_focus() {
		return;
	}
	let Some(range) = output.cursor_range else {
		return;
	};
	let origin = output.galley_pos.to_vec2() - egui::vec2(output.galley.rect.left(), 0.0);
	let clip = output.text_clip_rect.intersect(ui.clip_rect());
	if !range.is_empty() {
		let [min, max] = range.sorted_cursors();
		let min_lc = output.galley.layout_from_cursor(min);
		let max_lc = output.galley.layout_from_cursor(max);
		let mut shapes = Vec::new();
		for ri in min_lc.row..=max_lc.row {
			let Some(placed) = output.galley.rows.get(ri) else {
				continue;
			};
			let left = if ri == min_lc.row {
				placed.row.x_offset(min_lc.column)
			} else {
				0.0
			};
			let right = if ri == max_lc.row {
				placed.row.x_offset(max_lc.column)
			} else {
				placed.row.size.x
			};
			let rect = egui::Rect::from_min_max(
				egui::pos2(placed.pos.x + left, placed.pos.y),
				egui::pos2(placed.pos.x + right, placed.pos.y + placed.row.size.y),
			)
			.translate(origin)
			.intersect(clip);
			if rect.is_positive() {
				shapes.push(egui::Shape::rect_filled(
					rect,
					0.0,
					visuals.selection.bg_fill,
				));
			}
		}
		ui.painter().set(slot, shapes);
	}
	let cursor_rect = output
		.galley
		.pos_from_cursor(range.primary)
		.translate(origin);
	let top = cursor_rect.center_top();
	let bottom = cursor_rect.center_bottom();
	if ui.input(|i| i.focused) && (clip.contains(top) || clip.contains(bottom)) {
		let painter = ui.painter().with_clip_rect(clip);
		paint_blinking_caret(ui, &painter, visuals, top, bottom, last_interaction_time);
	}
}

/// True when `text` or this frame's input events could activate right-to-left visual layout,
/// eliminating 1-frame cursor/selection lag when inserting the first Arabic char, undoing, or
/// deleting the last Arabic char.
pub(crate) fn will_predict_bidi(text: &str, ctx: &egui::Context, id: egui::Id) -> bool {
	if has_rtl(text) {
		return true;
	}
	let focused = ctx.memory(|m| m.has_focus(id));
	ctx.input(|i| {
		i.events.iter().any(|event| match event {
			egui::Event::Text(s) | egui::Event::Paste(s) => has_rtl(s),
			egui::Event::Ime(
				egui::ImeEvent::Preedit { text, .. } | egui::ImeEvent::Commit(text),
			) => has_rtl(text),
			egui::Event::Key {
				key: egui::Key::Z | egui::Key::Y,
				pressed: true,
				modifiers,
				..
			} => focused && (modifiers.command || modifiers.ctrl),
			_ => false,
		})
	})
}

/// Single-line label galley with RTL support for truncated pills, headers and hints.
pub(crate) fn singleline_galley(
	ui: &egui::Ui,
	text: String,
	font: egui::FontId,
	color: Color32,
	max_width: f32,
) -> Arc<egui::Galley> {
	if has_rtl(&text) {
		let make_rtl = |sample: &str| {
			crate::rtl::layout(
				ui.ctx(),
				&[crate::rtl::Span {
					text: sample.to_owned(),
					format: TextFormat::simple(font.clone(), color),
					action: 0,
					object: None,
					copy: false,
				}],
				f32::INFINITY,
			)
			.and_then(|rtl| rtl.lines.first().map(|line| line.galley.clone()))
		};
		if let Some(full) = make_rtl(&text) {
			if !max_width.is_finite() || full.size().x <= max_width.max(1.0) {
				return full;
			}
			let limit = max_width.max(1.0);
			let boundaries: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
			let mut lo = 0usize;
			let mut hi = boundaries.len().saturating_sub(1);
			let mut best = make_rtl("\u{2026}").unwrap_or(full);
			while lo < hi {
				let mid = lo + (hi - lo + 1) / 2;
				let candidate = format!("{}\u{2026}", &text[..boundaries[mid]]);
				if let Some(galley) = make_rtl(&candidate)
					&& galley.size().x <= limit
				{
					best = galley;
					lo = mid;
				} else {
					hi = mid - 1;
				}
			}
			return best;
		}
	}
	let mut job = LayoutJob::simple_singleline(text, font, color);
	job.wrap.max_width = max_width.max(1.0);
	job.wrap.max_rows = 1;
	job.wrap.break_anywhere = true;
	ui.fonts_mut(|f| f.layout_job(job))
}

/// Non-selectable single-line label with RTL shaping and truncation when needed.
pub(crate) fn singleline_label(
	ui: &mut egui::Ui,
	text: &str,
	font: egui::FontId,
	color: Color32,
	max_width: f32,
	sense: egui::Sense,
) -> egui::Response {
	if has_rtl(text) {
		let galley = singleline_galley(ui, text.to_owned(), font, color, max_width);
		ui.add(egui::Label::new(galley).sense(sense).selectable(false))
	} else {
		ui.add(
			egui::Label::new(egui::RichText::new(text).font(font).color(color))
				.truncate()
				.sense(sense)
				.selectable(false),
		)
	}
}

/// Multi-segment single-line galley with RTL support (e.g. typing indicator `"<name> is typing..."`).
pub(crate) fn singleline_spans_galley(
	ui: &egui::Ui,
	spans: &[(String, egui::FontId, Color32)],
	max_width: f32,
) -> Arc<egui::Galley> {
	if spans.iter().any(|(text, _, _)| has_rtl(text)) {
		let rtl_spans: Vec<_> = spans
			.iter()
			.map(|(text, font, color)| crate::rtl::Span {
				text: text.clone(),
				format: TextFormat::simple(font.clone(), *color),
				action: 0,
				object: None,
				copy: false,
			})
			.collect();
		if let Some(rtl) = crate::rtl::layout(ui.ctx(), &rtl_spans, f32::INFINITY)
			&& let Some(line) = rtl.lines.first()
		{
			return line.galley.clone();
		}
	}
	let mut job = LayoutJob::default();
	job.wrap.max_width = max_width.max(1.0);
	job.wrap.max_rows = 1;
	job.wrap.break_anywhere = true;
	for (text, font, color) in spans {
		job.append(text, 0.0, TextFormat::simple(font.clone(), *color));
	}
	ui.fonts_mut(|f| f.layout_job(job))
}

const MAX_BIDI_FIELDS: usize = 16;

#[derive(Default)]
struct BidiInputCache {
	entries: std::collections::VecDeque<(egui::Id, BidiCore)>,
}

fn input_cache_id() -> egui::Id {
	egui::Id::unique("bounded-bidi-text-input-cache")
}

/// Drop cached standalone bidi `TextEdit` states (called on session/channel switches).
pub(crate) fn clear_input_cache(ctx: &egui::Context) {
	ctx.data_mut(|data| {
		data.remove::<Arc<std::sync::Mutex<BidiInputCache>>>(input_cache_id());
	});
}

fn take_field_core(ctx: &egui::Context, id: egui::Id) -> BidiCore {
	let cache = ctx.data_mut(|data| {
		data.get_temp::<Arc<std::sync::Mutex<BidiInputCache>>>(input_cache_id())
			.unwrap_or_else(|| {
				let created = Arc::new(std::sync::Mutex::new(BidiInputCache::default()));
				data.insert_temp(input_cache_id(), created.clone());
				created
			})
	});
	let mut guard = cache.lock().expect("bidi field cache lock");
	if let Some(pos) = guard.entries.iter().position(|(eid, _)| *eid == id) {
		guard.entries.remove(pos).unwrap().1
	} else {
		BidiCore::default()
	}
}

fn put_field_core(ctx: &egui::Context, id: egui::Id, core: BidiCore) {
	let Some(cache) =
		ctx.data(|data| data.get_temp::<Arc<std::sync::Mutex<BidiInputCache>>>(input_cache_id()))
	else {
		return;
	};
	let mut guard = cache.lock().expect("bidi field cache lock");
	if let Some(pos) = guard.entries.iter().position(|(eid, _)| *eid == id) {
		guard.entries.remove(pos);
	}
	while guard.entries.len() >= MAX_BIDI_FIELDS {
		guard.entries.pop_front();
	}
	guard.entries.push_back((id, core));
}

/// Wrap any plain `TextEdit` (`singleline` or `multiline`) with full Arabic/RTL shaping, visual
/// cursor navigation, pointer hit-testing, and flicker-free selection/caret painting.
pub(crate) fn show_text_edit(
	ui: &mut egui::Ui,
	id: egui::Id,
	text: &mut dyn egui::TextBuffer,
	multiline: bool,
	build: impl for<'t> FnOnce(egui::TextEdit<'t>) -> egui::TextEdit<'t>,
) -> egui::text_edit::TextEditOutput {
	let font = egui::TextStyle::Body.resolve(ui.style());
	let color = ui.visuals().text_color();
	show_text_edit_with_job(
		ui,
		id,
		text,
		multiline,
		move |_ui, value, width| {
			let mut job = LayoutJob::simple(value.to_owned(), font.clone(), color, width);
			if !multiline {
				job.wrap.max_rows = 1;
				job.wrap.max_width = f32::INFINITY;
			}
			job
		},
		build,
	)
}

/// Same as [`show_text_edit`], preserving custom highlighting sections (e.g. search filter pills).
pub(crate) fn show_text_edit_with_job(
	ui: &mut egui::Ui,
	id: egui::Id,
	text: &mut dyn egui::TextBuffer,
	multiline: bool,
	mut make_job: impl FnMut(&egui::Ui, &str, f32) -> LayoutJob,
	build: impl for<'t> FnOnce(egui::TextEdit<'t>) -> egui::TextEdit<'t>,
) -> egui::text_edit::TextEditOutput {
	let ctx = ui.ctx().clone();
	let mut core = take_field_core(&ctx, id);
	let ime_active = ctx.input(|i| {
		i.events.iter().any(
			|e| matches!(e, egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) if !text.is_empty()),
		)
	});
	core.prepare_navigation(&ctx, id, text.as_str(), ime_active, &[]);
	let hid_native = will_predict_bidi(text.as_str(), &ctx, id);
	let outer_visuals = ui.visuals().clone();
	let selection_slot = ui.painter().add(egui::Shape::Noop);
	let mut layouter =
		|ui: &egui::Ui, buffer: &dyn egui::TextBuffer, width: f32| -> Arc<egui::Galley> {
			let value = buffer.as_str();
			let job = make_job(ui, value, width);
			let mut key = DefaultHasher::new();
			job.text.hash(&mut key);
			job.wrap.max_width.to_bits().hash(&mut key);
			job.wrap.max_rows.hash(&mut key);
			for s in &job.sections {
				s.byte_range.start.0.hash(&mut key);
				s.byte_range.end.0.hash(&mut key);
				s.format.color.hash(&mut key);
				s.format.background.hash(&mut key);
				s.format.font_id.hash(&mut key);
			}
			crate::fonts::revision(ui.ctx()).hash(&mut key);
			ui.ctx().fonts(|f| f.layout_generation()).hash(&mut key);
			ui.ctx().pixels_per_point().to_bits().hash(&mut key);
			let hash = key.finish();
			if let Some((cached, galley)) = &core.cache
				&& *cached == hash
			{
				return galley.clone();
			}
			let mut galley = ui.fonts_mut(|f| f.layout_job(job));
			let reordered = has_rtl(value)
				.then(|| {
					reorder(
						ui,
						&mut galley,
						if multiline { width } else { f32::INFINITY },
					)
				})
				.flatten();
			core.bidi = reordered.map(|(cells, rtl)| Bidi::new(cells, &rtl, &galley, &[]));
			core.cache = Some((hash, galley.clone()));
			galley
		};
	let mut output = ui
		.scope(|ui| {
			if hid_native {
				let visuals = ui.visuals_mut();
				visuals.text_cursor.stroke.color = Color32::TRANSPARENT;
				visuals.selection.bg_fill = Color32::TRANSPARENT;
			}
			let edit = if multiline {
				egui::TextEdit::multiline(&mut *text)
			} else {
				egui::TextEdit::singleline(&mut *text)
			};
			build(edit).id(id).layouter(&mut layouter).show(ui)
		})
		.inner;
	core.bidi_pointer(&mut output, &ctx);
	core.finish_paint(ui, &mut output, selection_slot, hid_native, &outer_visuals);
	put_field_core(&ctx, id, core);
	output
}

impl Layout {
	/// Whether `text` is laid out in visual order, so the composer draws its caret and selection.
	#[cfg(test)]
	pub fn bidi_active(&self, text: &str) -> bool {
		self.core.bidi_active(text)
	}

	/// Prepare keyboard navigation (`ArrowLeft/Right/Up/Down`, `Home/End`) before `TextEdit` runs.
	pub fn prepare_navigation(
		&mut self,
		ctx: &egui::Context,
		id: egui::Id,
		text: &str,
		ime_active: bool,
	) {
		self.core
			.prepare_navigation(ctx, id, text, ime_active, &self.inlines);
	}

	/// Map the pointer to logical characters for a right-to-left draft.
	pub fn bidi_pointer(
		&mut self,
		output: &mut egui::text_edit::TextEditOutput,
		ctx: &egui::Context,
	) {
		self.core.bidi_pointer(output, ctx);
	}

	/// Finish painting the composer's bidi caret/selection (or LTR fallback) and fix IME/scroll.
	pub fn finish_paint(
		&mut self,
		ui: &mut egui::Ui,
		output: &mut egui::text_edit::TextEditOutput,
		slot: egui::layers::ShapeIdx,
		hid_native: bool,
		outer_visuals: &egui::Visuals,
	) {
		self.core
			.finish_paint(ui, output, slot, hid_native, outer_visuals);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn users() -> Vec<User> {
		vec![User {
			id: model::Id(42),
			name: "Zoë".into(),
			avatar: None,
			webhook: false,
			kind: Default::default(),
			discriminator: 0,
			username: None,
			primary_guild: None,
			cosmetics: None,
		}]
	}

	/// #918: right-to-left drafts read in visual order, and the caret and pointer follow it.
	#[test]
	fn rtl_drafts_are_drawn_in_visual_order_with_logical_hit_testing() {
		let ctx = egui::Context::default();
		crate::fonts::install(&ctx);
		let mut avatars = Avatars::default();
		let mut layout = Layout::default();
		let mut check = |text: &str, rtl_paragraph: bool| {
			let mut galley = None;
			ctx.run_ui(Default::default(), |ui| {
				galley =
					Some(layout.galley(ui, text, 300.0, &[], &[], &[], false, &mut avatars, true));
			})
			.drop_without_applying_deltas();
			let galley = galley.unwrap();
			let bidi = layout
				.core
				.bidi
				.as_ref()
				.expect("RTL draft uses visual order");
			let row = &galley.rows[0];
			assert_eq!(
				row.row.glyphs.len(),
				text.chars().count(),
				"one glyph per char"
			);
			assert_eq!(
				row.pos.x > 0.0,
				rtl_paragraph,
				"only RTL paragraphs align right"
			);
			let x = |index: usize| bidi.caret(index).unwrap().1;
			let len = text.chars().count();
			if rtl_paragraph {
				assert!(x(0) > x(len), "reading starts at the right");
				assert_eq!(bidi.index_at(egui::pos2(x(0) + 20.0, 5.0)), 0);
				assert_eq!(bidi.index_at(egui::pos2(x(len) - 20.0, 5.0)), len);
			} else {
				// "abc שלום": Latin first on the left, then Hebrew right to left.
				assert!(x(0) < x(4), "Latin precedes the Hebrew word");
				assert!(x(4) > x(8), "the Hebrew word reads right to left");
				assert_eq!(bidi.index_at(egui::pos2(x(4) - 1.0, 5.0)), 4);
			}
		};
		check("مرحبا بك", true);
		check("abc שלום", false);
		// Wrapped and multi-line drafts: egui maps the cursor through one glyph per char.
		let text = "مرحبا بك في سيرين مرحبا بك في سيرين\n\nabc שלום";
		let mut galley = None;
		ctx.run_ui(Default::default(), |ui| {
			galley = Some(layout.galley(ui, text, 60.0, &[], &[], &[], false, &mut avatars, true));
		})
		.drop_without_applying_deltas();
		let galley = galley.unwrap();
		assert!(galley.rows.len() > 4, "the Arabic paragraph wraps");
		let mapped: usize = galley
			.rows
			.iter()
			.map(|row| row.row.glyphs.len() + usize::from(row.ends_with_newline))
			.sum();
		assert_eq!(mapped, text.chars().count());
		// A line after a right-to-left paragraph keeps its own direction.
		let text = "مرحبا\nabc";
		let mut galley = None;
		ctx.run_ui(Default::default(), |ui| {
			galley = Some(layout.galley(ui, text, 300.0, &[], &[], &[], false, &mut avatars, true));
		})
		.drop_without_applying_deltas();
		let rows = &galley.unwrap().rows;
		assert!(rows[0].pos.x > 0.0 && rows[1].pos.x == 0.0);
		assert!(paragraph_rtl(text, 0) && !paragraph_rtl(text, 6));
		ctx.run_ui(Default::default(), |ui| {
			layout.galley(ui, "plain", 300.0, &[], &[], &[], false, &mut avatars, true);
		})
		.drop_without_applying_deltas();
		assert!(
			layout.core.bidi.is_none(),
			"left-to-right drafts keep egui's own caret"
		);
	}

	/// An unchanged draft reuses its galley until the glyph atlas starts over.
	#[test]
	fn unchanged_draft_is_laid_out_again_after_an_atlas_reset() {
		let ctx = egui::Context::default();
		crate::fonts::install(&ctx);
		let mut avatars = Avatars::default();
		let mut layout = Layout::default();
		let mut draft = || {
			let mut galley = None;
			ctx.run_ui(Default::default(), |ui| {
				galley = Some(layout.galley(
					ui,
					"draft",
					300.0,
					&[],
					&[],
					&[],
					false,
					&mut avatars,
					true,
				));
			})
			.drop_without_applying_deltas();
			galley.unwrap()
		};
		let first = draft();
		assert!(Arc::ptr_eq(&first, &draft()));
		// New text options start the atlas over, as its byte budget does.
		ctx.global_style_mut(|style| {
			style.visuals.text_options.font_hinting = !style.visuals.text_options.font_hinting;
		});
		assert!(!Arc::ptr_eq(&first, &draft()));
	}

	/// A double-click selects the whole Arabic word, and the release frames keep it.
	#[test]
	fn rtl_double_click_keeps_the_whole_word() {
		let ctx = egui::Context::default();
		crate::fonts::install(&ctx);
		let mut avatars = Avatars::default();
		let mut layout = Layout::default();
		let mut text = "مرحبا بك".to_owned();
		let id = egui::Id::unique("rtl-double-click");
		let point = std::cell::Cell::new(egui::Pos2::ZERO);
		let mut frame = |time: f64, events: Vec<egui::Event>| {
			ctx.run_ui(
				egui::RawInput {
					time: Some(time),
					events,
					..Default::default()
				},
				|ui| {
					let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, width| {
						layout.galley(
							ui,
							buffer.as_str(),
							width,
							&[],
							&[],
							&[],
							false,
							&mut avatars,
							true,
						)
					};
					let mut output = egui::TextEdit::multiline(&mut text)
						.id(id)
						.layouter(&mut layouter)
						.show(ui);
					layout.bidi_pointer(&mut output, ui.ctx());
					// Inside the first word "مرحبا", between its second and third letters.
					let (row, x) = layout.core.bidi.as_ref().unwrap().caret(2).unwrap();
					point.set(egui::pos2(x, row.center().y) + output.galley_pos.to_vec2());
				},
			)
			.drop_without_applying_deltas();
		};
		frame(0.0, vec![]);
		let button = |pressed| egui::Event::PointerButton {
			pos: point.get(),
			button: egui::PointerButton::Primary,
			pressed,
			modifiers: egui::Modifiers::NONE,
		};
		frame(
			1.0,
			vec![egui::Event::PointerMoved(point.get()), button(true)],
		);
		for (time, pressed) in [(1.05, false), (1.1, true), (1.15, false)] {
			frame(time, vec![button(pressed)]);
		}
		frame(1.2, vec![]);
		let range = egui::text_edit::TextEditState::load(&ctx, id)
			.and_then(|state| state.cursor.char_range())
			.unwrap();
		let [min, max] = range.sorted_cursors();
		assert_eq!((min.index.0, max.index.0), (0, 5), "the whole first word");
	}

	#[test]
	fn native_delete_undo_copy_and_ime_keep_wire_mentions() {
		let ctx = egui::Context::default();
		let mut avatars = Avatars::default();
		let mut layout = Layout::default();
		let mut text = "hi <@42>!".to_owned();
		let id = egui::Id::unique("rich-editor-test");
		let mut frame = |time, events: Vec<egui::Event>, cursor: Option<usize>| {
			if let Some(cursor) = cursor {
				ctx.memory_mut(|m| m.request_focus(id));
				let mut state = egui::text_edit::TextEditState::load(&ctx, id).unwrap_or_default();
				state
					.cursor
					.set_char_range(Some(egui::text::CCursorRange::one(CCursor::new(cursor))));
				state.store(&ctx, id);
			}
			let copying = events
				.iter()
				.any(|event| matches!(event, egui::Event::Copy));
			let composing = events
				.iter()
				.any(|event| matches!(event, egui::Event::Ime(_)));
			let output = ctx.run_ui(
				egui::RawInput {
					time: Some(time),
					events,
					..Default::default()
				},
				|ui| {
					layout.galley(
						ui,
						&text,
						300.0,
						&users(),
						&[],
						&[],
						false,
						&mut avatars,
						true,
					);
					layout.select_deleted_inline(&ctx, id);
					let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, width| {
						layout.galley(
							ui,
							buffer.as_str(),
							width,
							&users(),
							&[],
							&[],
							false,
							&mut avatars,
							true,
						)
					};
					let mut edit = egui::TextEdit::multiline(&mut text)
						.id(id)
						.layouter(&mut layouter)
						.show(ui);
					if !composing {
						layout.snap_cursor(&mut edit, &ctx);
					}
					layout.paint(ui, &edit, &mut avatars);
				},
			);
			if copying {
				assert!(output.platform_output.commands.iter().any(|command| {
					matches!(command, egui::OutputCommand::CopyText(copied) if copied == &text)
				}));
			}
			output.drop_without_applying_deltas();
			text.clone()
		};
		let key = |key, modifiers| egui::Event::Key {
			key,
			physical_key: None,
			pressed: true,
			repeat: false,
			modifiers,
		};
		assert_eq!(frame(0.0, vec![], Some(8)), "hi <@42>!");
		assert_eq!(
			frame(
				1.0,
				vec![key(egui::Key::Backspace, egui::Modifiers::NONE)],
				None
			),
			"hi !"
		);
		assert_eq!(
			frame(2.0, vec![key(egui::Key::Z, egui::Modifiers::COMMAND)], None),
			"hi <@42>!"
		);
		frame(
			2.1,
			vec![key(egui::Key::ArrowLeft, egui::Modifiers::NONE)],
			Some(8),
		);
		assert_eq!(
			egui::text_edit::TextEditState::load(&ctx, id)
				.unwrap()
				.cursor
				.char_range()
				.unwrap()
				.primary
				.index
				.0,
			3
		);
		frame(
			2.2,
			vec![key(egui::Key::ArrowRight, egui::Modifiers::NONE)],
			None,
		);
		assert_eq!(
			egui::text_edit::TextEditState::load(&ctx, id)
				.unwrap()
				.cursor
				.char_range()
				.unwrap()
				.primary
				.index
				.0,
			8
		);
		assert_eq!(
			frame(3.0, vec![egui::Event::Text("č".into())], Some(8)),
			"hi <@42>č!"
		);
		assert_eq!(
			frame(
				4.0,
				vec![egui::Event::Ime(egui::ImeEvent::Preedit {
					text: "ni".into(),
					active_range_chars: None,
				})],
				Some(9)
			),
			"hi <@42>čni!"
		);
		assert_eq!(
			frame(
				5.0,
				vec![egui::Event::Ime(egui::ImeEvent::Commit("你".into()))],
				None
			),
			"hi <@42>č你!"
		);
		assert_eq!(
			frame(
				6.0,
				vec![
					key(egui::Key::A, egui::Modifiers::COMMAND),
					egui::Event::Copy
				],
				None
			),
			"hi <@42>č你!"
		);
	}

	#[test]
	fn arabic_first_last_char_prediction_crlf_and_zero_width_stay_in_sync() {
		let ctx = egui::Context::default();
		crate::fonts::install(&ctx);
		let mut avatars = Avatars::default();
		let mut layout = Layout::default();
		let id = egui::Id::unique("arabic-crlf-sync");
		// Frame 0 prediction catches the very first incoming Arabic keystroke.
		ctx.run_ui(
			egui::RawInput {
				events: vec![egui::Event::Text("م".into())],
				..Default::default()
			},
			|ui| {
				assert!(will_predict_bidi("", ui.ctx(), id));
				let text = "مرحبا\r\n\u{200f}\r\nبكم".to_owned();
				let galley =
					layout.galley(ui, &text, 300.0, &[], &[], &[], false, &mut avatars, true);
				assert!(
					layout.bidi_active(&text),
					"CRLF and invisible controls keep bidi enabled"
				);
				assert_eq!(
					galley.rect.left(),
					0.0,
					"right-aligned RTL galley keeps min.x == 0.0 so TextEdit does not shift glyphs off the caret"
				);
				let mapped: usize = galley
					.rows
					.iter()
					.map(|row| row.row.glyphs.len() + usize::from(row.ends_with_newline))
					.sum();
				assert_eq!(mapped, text.chars().count());
				let bidi = layout.core.bidi.as_ref().unwrap();
				for i in 0..=text.chars().count() {
					assert!(bidi.caret(i).is_some(), "caret exists at char {i}");
				}
				assert!(will_predict_bidi(&text, ui.ctx(), id));
			},
		)
		.drop_without_applying_deltas();
	}

	#[test]
	fn arabic_mixed_navigation_vertical_arrows_and_singleline_input() {
		let ctx = egui::Context::default();
		crate::fonts::install(&ctx);
		let mut avatars = Avatars::default();
		let mut layout = Layout::default();
		let mixed = "مرحبا hello بكم";
		ctx.run_ui(Default::default(), |ui| {
			let _ = layout.galley(ui, mixed, 400.0, &[], &[], &[], false, &mut avatars, true);
		})
		.drop_without_applying_deltas();
		let bidi = layout.core.bidi.as_ref().unwrap();
		// Stepping left visually across mixed Arabic/English never moves caret X rightward.
		let mut cursor = CCursor::new(0);
		let mut xs = vec![bidi.caret_cursor(cursor).unwrap().1];
		for _ in 0..mixed.chars().count() {
			cursor = bidi.step_horizontal(mixed, cursor, false, false, &[]);
			xs.push(bidi.caret_cursor(cursor).unwrap().1);
		}
		for pair in xs.windows(2) {
			assert!(
				pair[1] <= pair[0] + 0.01,
				"ArrowLeft visually moved right: {} -> {}",
				pair[0],
				pair[1]
			);
		}
		// Multiline vertical navigation keeps approximate visual X between Arabic rows.
		let multi = "مرحبا بك في سطر\nالسطر الثاني هنا";
		ctx.run_ui(Default::default(), |ui| {
			let _ = layout.galley(ui, multi, 400.0, &[], &[], &[], false, &mut avatars, true);
		})
		.drop_without_applying_deltas();
		let bidi = layout.core.bidi.as_ref().unwrap();
		let top_mid = CCursor::new(5);
		let (_, top_x) = bidi.caret_cursor(top_mid).unwrap();
		let down = bidi.step_vertical(top_mid, true, top_x, &[]);
		let row1 = bidi.row_index_for_cursor(down).unwrap();
		assert_eq!(row1, 1);
		let up = bidi.step_vertical(down, false, top_x, &[]);
		assert_eq!(bidi.row_index_for_cursor(up), Some(0));

		// Standalone singleline bidi edit handles Arabic text and arrow editing cleanly.
		let mut single = "مرحبا".to_owned();
		let single_id = egui::Id::unique("singleline-arabic-edit");
		ctx.memory_mut(|m| m.request_focus(single_id));
		let frame = |time, text: &mut String, events: Vec<egui::Event>| {
			ctx.run_ui(
				egui::RawInput {
					time: Some(time),
					events,
					..Default::default()
				},
				|ui| {
					let _ = show_text_edit(ui, single_id, text, false, |e| e);
				},
			)
			.drop_without_applying_deltas();
		};
		frame(0.0, &mut single, vec![]);
		frame(0.1, &mut single, vec![egui::Event::Text("!".into())]);
		assert!(single.contains('!') && has_rtl(&single));

		let mut truncated = None;
		ctx.run_ui(Default::default(), |ui| {
			truncated = Some(singleline_galley(
				ui,
				"صلي على النبي وبارك عليه دائما وأبدا في كل وقت".into(),
				egui::FontId::proportional(12.0),
				egui::Color32::WHITE,
				90.0,
			));
		})
		.drop_without_applying_deltas();
		let truncated = truncated.unwrap();
		assert!(truncated.size().x <= 91.0 && truncated.job.text.contains('\u{2026}'));
	}
}
