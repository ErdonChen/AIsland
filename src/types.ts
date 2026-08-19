export type IslandMode = "collapsed" | "expanded";
export type IslandExpansionMotion = "elastic" | "smooth" | "swift";
export type IslandPage =
  | "home"
  | "note"
  | "clipboard"
  | "monitor"
  | "notify"
  | "settings";

export type InitialState = {
  scale: number;
  dpi: number;
  mode: IslandMode;
  collapsedWidth: number;
  expandedWidth: number;
  expandedHeight: number;
  tucked: boolean;
  rasterizationError: string | null;
};
