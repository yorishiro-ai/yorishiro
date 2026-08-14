import { useEffect, useState } from "react";

/// Tracks whether the dark theme is active, for the places that need the *value* rather than
/// a CSS token -- canvas colours passed to React Flow as inline style, which cannot resolve a
/// custom property.
///
/// Watches the class on `<html>` instead of reading `useTheme`, so it stays correct however
/// the class is set: the toggle, the initial `prefers-color-scheme` resolution, or anything
/// else that flips it.
export function useIsDarkMode(): boolean {
  const [isDark, setIsDark] = useState(
    () => typeof document !== "undefined" && document.documentElement.classList.contains("dark"),
  );

  useEffect(() => {
    const el = document.documentElement;
    const observer = new MutationObserver(() => {
      setIsDark(el.classList.contains("dark"));
    });
    observer.observe(el, { attributes: true, attributeFilter: ["class"] });
    return () => observer.disconnect();
  }, []);

  return isDark;
}
