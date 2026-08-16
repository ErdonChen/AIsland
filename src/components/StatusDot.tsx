type StatusDotProps = { color: string; pulse?: boolean };

export default function StatusDot({ color, pulse = false }: StatusDotProps) {
  return (
    <span
      className={`status-dot${pulse ? " status-dot--pulse" : ""}`}
      style={{ background: color }}
      aria-hidden="true"
    />
  );
}
