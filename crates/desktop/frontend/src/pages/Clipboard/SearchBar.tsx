import { Search } from "lucide-react";
import { useT } from "@/lib/i18n";

export default function SearchBar({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  const t = useT();
  return (
    <div className="flex items-center gap-2 px-2.5 py-1.5 bg-muted rounded-lg border border-border/60 focus-within:border-voice/40 transition-colors">
      <Search className="w-3.5 h-3.5 text-muted-foreground flex-shrink-0" />
      <input
        type="text"
        autoCapitalize="off"
        autoCorrect="off"
        spellCheck={false}
        placeholder={t("clipboard.search")}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="flex-1 bg-transparent text-xs outline-none placeholder:text-muted-foreground/60"
        data-tauri-drag-region={false}
      />
    </div>
  );
}
