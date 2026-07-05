import type { XY } from "~/utils/tauri";

// Font sizes are authored in pixels against a 1080p-tall reference frame;
// the renderer scales them by outputHeight / 1080 (crates/rendering/text.rs).
export const TEXT_REFERENCE_HEIGHT = 1080;
export const TEXT_FONT_SIZE_MIN = 8;
export const TEXT_FONT_SIZE_MAX = 400;

export type TextSegment = {
	start: number;
	end: number;
	track: number;
	enabled: boolean;
	content: string;
	center: XY<number>;
	size: XY<number>;
	fontFamily: string;
	fontSize: number;
	fontWeight: number;
	italic: boolean;
	color: string;
	fadeDuration: number;
};

export const defaultTextSegment = (
	start: number,
	end: number,
): TextSegment => ({
	start,
	end,
	track: 0,
	enabled: true,
	content: "Text",
	center: { x: 0.5, y: 0.5 },
	// Rough hug of "Text" at 48px/1080p; the canvas overlay measures and
	// fits the box to the exact glyph bounds as soon as it renders.
	size: { x: 0.1, y: 0.055 },
	fontFamily: "sans-serif",
	fontSize: 48,
	fontWeight: 700,
	italic: false,
	color: "#ffffff",
	fadeDuration: 0.15,
});
