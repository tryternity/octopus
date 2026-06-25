function App() {
  const label =
    (window as any).__TAURI__?.window?.getCurrentWindow?.()?.label ?? "unknown";
  return (
    <div className="p-4 text-foreground">
      <p className="text-sm text-muted-foreground">Window label:</p>
      <p className="text-lg font-medium">{label}</p>
    </div>
  );
}

export default App;
