export function ToolButton({ active, onClick, label, icon }: { active?: boolean; onClick: (e: React.MouseEvent<HTMLButtonElement>) => void; label: string; icon: React.ReactNode }) {
  return (
    <button
      onClick={onClick}
      title={label}
      style={{
        width: 32, height: 32,
        display: "flex", alignItems: "center", justifyContent: "center",
        borderRadius: 6,
        border: "none",
        background: active ? "var(--color-voice)" : "transparent",
        cursor: "pointer",
        transition: "background 0.15s",
      }}
    >
      {icon}
    </button>
  );
}
