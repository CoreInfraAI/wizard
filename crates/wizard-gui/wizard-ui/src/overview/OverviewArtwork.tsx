import { useId } from "react";
import { OverviewPenLogo } from "./OverviewPenLogo";

// Copybook forms: fine entry strokes, rounded turns and a little pressure on downstrokes.
// Each continuous pen trace also reveals its pressure strokes during the entrance animation.
const numerals = {
  account: {
    trace: "M14 25.7C18.5 22.2 24 17.3 29.6 11.4C29.3 23 28.6 41.5 28.1 54.1",
    pressure: ["M29.9 11.2C30 24 29.5 43.2 28.9 54.1C28.8 55.3 27 55.4 26.9 54C27.4 41 28.3 23.3 28.9 11.7Z"],
  },
  install: {
    trace: "M13.5 23C13 16.1 17.7 11.1 24.4 10.9C31.1 10.7 36.1 14.7 35.4 21.4C34.6 29.1 25 35.1 19 40.4C13.6 45.3 10.3 49.9 9.9 53.7C16.6 49.7 23 51.7 28.8 53.3C33 54.4 37.1 53.2 39.3 50.3",
    pressure: [
      "M26 10.4C32.7 10.1 37.8 14.9 36.9 21.6C36.1 29.8 25.8 36.1 19.5 41L18.6 39.8C24.8 34 32.8 28.3 33.5 21.3C34.2 15.9 30.5 12 25.9 11.7Z",
      "M9.5 53.1C16.2 48.2 23 50.1 29.1 52.2C33 53.5 36.6 52.5 39.3 49.8L39.5 50.7C36.6 55 32.9 55.9 28.6 54.4C22.2 52.2 15.8 51.1 10.4 54.4Z",
    ],
  },
  configure: {
    trace: "M13 19C14.5 13.7 19 10.4 25.4 10.7C32.7 10.8 36.4 15.5 35.3 21.6C34.2 27.6 28 31.3 22 32.2C29.4 30.5 36.4 34.2 36.3 41.5C36 49.4 29.6 55.4 21.5 55.1C15.2 55 10.9 51.2 10.3 46",
    pressure: [
      "M25.3 10C33.1 10 38.2 15.2 36.8 21.9C35.3 28.5 28.5 31.8 22 32.8L21.8 31.6C27.7 30.2 32.7 26.9 33.7 21.3C34.7 15.7 31.3 11.7 25.4 11.4Z",
      "M22 31.5C29.8 29.9 38.2 33.3 37.9 41.7C37.5 50.3 29.7 56.3 21.4 55.8L21.6 54.4C29 54.7 34.6 48.3 34.8 41.4C35 35.1 29 31.7 22.2 32.9Z",
    ],
  },
};

/** Pen-written step numbers and quiet graphite drawings, kept crisp at any scale. */
export function OverviewArtwork({ kind, connected = false, step = true, disabled, label, describedBy, onClick }: {
  kind: keyof typeof numerals;
  connected?: boolean;
  step?: boolean;
  disabled: boolean;
  label: string;
  describedBy?: string;
  onClick: () => void;
}) {
  const numeralMask = useId();
  const numeral = numerals[kind];
  return <button type="button" className="overview-art" data-connected={connected}
    disabled={disabled} aria-label={label} aria-describedby={describedBy} onClick={onClick}>
    <span className="overview-art-grid" aria-hidden="true" />
    {step && <svg className="overview-step-number" viewBox="0 0 56 68" fill="none" strokeLinecap="round" strokeLinejoin="round" focusable="false" aria-hidden="true">
      <defs>
        <mask id={numeralMask} maskUnits="userSpaceOnUse" x="0" y="0" width="56" height="68">
          <path className="overview-number-trace" d={numeral.trace} pathLength={1} stroke="white" strokeWidth="8" />
        </mask>
      </defs>
      <g transform="translate(10 0) skewX(-12)">
        <g mask={`url(#${numeralMask})`}>
          <path d={numeral.trace} stroke="currentColor" strokeWidth="1.45" />
          {numeral.pressure.map((d) => <path key={d} d={d} fill="currentColor" />)}
        </g>
      </g>
      <path className="overview-number-dot" d="M44.6 53.2C46.6 52.9 47.2 55.5 45.2 56C43.4 56.2 43.1 53.7 44.6 53.2Z" fill="currentColor" />
    </svg>}
    <svg className="overview-sketch" viewBox="0 0 176 128" fill="none" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" strokeLinejoin="round" focusable="false" aria-hidden="true">
      {kind === "account" && <>
        <g transform="rotate(-4 88 58)">
          <path className="overview-sketch-paper" d="M24 16Q80 14 141 17Q146 17 146 22L145 91Q145 96 140 96L24 95Q19 95 20 89L20 23Q19 17 24 16Z" />
          <path d="M21 32Q81 31 145 33" />
          <path className="overview-sketch-faint" d="M25 99Q85 100 138 98M149 23L149 86" />
          <path d="M29 24L30 24M36 24L37 24M43 24L44 24" strokeWidth="2" />
          <path d="M53 48C39 40 31 61 43 65C47 67 51 65 54 63" strokeWidth="2.6" />
          <path d="M66 49Q92 48 124 50M65 59L106 59M33 80Q61 79 90 80M102 80L130 80" className="overview-sketch-soft" />
        </g>
        <g className="overview-sketch-identity">
          <path className="overview-sketch-paper" d="M54 87Q100 85 158 87Q163 87 162 93L162 111Q162 116 156 115L53 114Q49 114 49 109L49 94Q48 88 54 87Z" />
          <path d="M62 97C62 92 70 92 70 97C70 103 62 102 62 97ZM58 109C58 101 74 101 75 109" />
          <path d="M85 101Q104 100 123 101" className="overview-sketch-soft" />
          <path d="M137 101L151 101M146 96L151 101L146 106" />
        </g>
      </>}
      {kind === "install" && <>
        <g className="overview-sketch-app overview-sketch-app-claude" transform="rotate(-7 51 49)">
          <OverviewPenLogo kind="claude" className="overview-sketch-claude" x={31} y={28} width={37} height={37} />
        </g>
        <g className="overview-sketch-app overview-sketch-app-openai" transform="rotate(6 118 58)">
          <OverviewPenLogo kind="openai" className="overview-sketch-openai" x={99} y={38} width={36} height={36} />
        </g>
        <path className="overview-sketch-download-arrow" d="M42 95.5Q41.1 102 42 111M36.7 105.6Q39.1 108.2 42 111Q44.1 108.4 47.3 105.1" />
        <path className="overview-sketch-paper" d="M60.5 99.5Q98 97.7 141.6 99.7Q143.2 103.4 141.2 111.2Q102 112.1 60 110.8Q58.8 105 60.5 99.5Z" />
        <path className="overview-sketch-progress" pathLength={1} d="M63.8 107.8L66.4 102.2L67.4 108.1L71 102.1L71.8 108.3L75.2 101.8L76.2 108.2L79.7 101.9L80.7 108.5L84.1 102.1L85.3 108.4L88.8 101.8L89.6 108.3L93.3 102L94.4 108.5L97.7 102.1L98.6 108.4L102 102.2L103.2 108.3L106.4 102.2L107.6 108.5L110 103" strokeWidth="1.1" />
      </>}
      {kind === "configure" && <>
        <g transform="rotate(-3 90 61)">
          <path className="overview-sketch-paper" d="M26 20Q83 18 146 21Q151 21 150 27L149 94Q149 99 144 98L25 97Q20 97 21 91L21 27Q20 21 26 20Z" />
          <path className="overview-sketch-faint" d="M26 102L143 102M154 28L154 91" />
          <path d="M35 39Q84 38 135 40M35 60Q85 59 135 60M35 80Q85 78 135 80" className="overview-sketch-soft" />
          <g className="overview-sketch-slider overview-sketch-slider-top"><path className="overview-sketch-paper" d="M100 34Q106 32 111 34L111 45Q106 47 100 45Z" /></g>
          <g className="overview-sketch-slider overview-sketch-slider-middle"><path className="overview-sketch-paper" d="M55 54Q60 53 65 54L65 65Q60 67 55 65Z" /></g>
          <g className="overview-sketch-slider overview-sketch-slider-bottom"><path className="overview-sketch-paper" d="M83 73Q88 72 93 73L93 85Q88 86 83 84Z" /></g>
        </g>
        <path d="M48 108Q65 118 91 114" className="overview-sketch-soft" />
        <path className="overview-sketch-paper" d="M109 91Q128 88 146 91Q152 92 151 99L150 114Q150 120 144 120L109 118Q103 118 104 111L104 97Q104 91 109 91Z" />
        <path d="M118 100L138 101M117 109L132 110" className="overview-sketch-soft" />
      </>}
    </svg>
  </button>;
}
