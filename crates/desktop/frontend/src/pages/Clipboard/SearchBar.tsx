import { Search } from "lucide-react";

export default function SearchBar({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <div className="flex items-center gap-2 px-2.5 py-1.5 bg-background rounded-lg border border-border/60 focus-within:border-primary/40 transition-colors">
      <Search className="w-3.5 h-3.5 text-muted-foreground/60 flex-shrink-0" />
      <input
        type="text"
        placeholder="搜索"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="flex-1 bg-transparent text-xs outline-none placeholder:text-muted-foreground/50"
        data-tauri-drag-region={false}
      />
    </div>
  );
}
