/** The GENORYN sign, drawn from the single brand geometry (scripts/brand/render_logo.py). */
export function GSign(props: { size?: number; class?: string; title?: string }) {
  return (
    <svg class={props.class} width={props.size ?? 28} height={props.size ?? 28} viewBox="0 0 256 256" role="img" aria-label={props.title ?? 'GENORYN'}>
      <path d="M186.00 128.00 A58 58 0 1 1 171.10 89.19" fill="none" stroke="currentColor" stroke-width="30" />
      <rect x="134" y="113" width="67" height="30" fill="currentColor" />
      <rect x="106" y="31" width="14" height="54" rx="2" fill="currentColor" />
      <rect x="106" y="171" width="14" height="54" rx="2" fill="currentColor" />
      <rect x="136" y="31" width="14" height="54" rx="2" fill="currentColor" />
      <rect x="136" y="171" width="14" height="54" rx="2" fill="currentColor" />
    </svg>
  );
}

/** Tile mark: sign on a rounded tile, inverting with the theme. */
export function GMark(props: { size?: number; class?: string }) {
  const s = props.size ?? 34;
  return (
    <span
      class={`gmark ${props.class ?? ''}`}
      style={{ width: `${s}px`, height: `${s}px`, 'border-radius': `${s * 0.22}px` }}
    >
      <GSign size={s * 0.82} />
    </span>
  );
}

/** Amount with the sign before it and the unit after, as the wallet does. */
export function GnrAmount(props: { value: string; unit?: boolean; class?: string }) {
  return (
    <span class={`gnr-amount ${props.class ?? ''}`}>
      <GSign size={14} class="gnr-sign" />
      <span>{props.value}</span>
      {props.unit !== false && <span class="gnr-unit">GNR</span>}
    </span>
  );
}
