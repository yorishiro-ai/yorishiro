// Edit this file (or bind-mount a replacement over it at deploy time, or point YSR_WEB_DIR at a
// directory with a replacement) if this dashboard needs to call a yorishiro-server that *isn't*
// the one serving these static files.
// Empty (the default) means same-origin, which is correct whenever the process serving this SPA
// is also the one serving the API -- true out of the box, regardless of bind address/port.
window.YORISHIRO_CONFIG = {
  apiBase: "",
};
