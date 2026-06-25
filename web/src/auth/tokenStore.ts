const REFRESH_KEY = "rst:refresh";
let accessToken: string | null = null;

export const tokenStore = {
  getAccessToken: () => accessToken,
  setAccessToken: (t: string | null) => { accessToken = t; },
  getRefreshToken: () => localStorage.getItem(REFRESH_KEY),
  setRefreshToken: (t: string | null) => {
    if (t) localStorage.setItem(REFRESH_KEY, t);
    else localStorage.removeItem(REFRESH_KEY);
  },
  clear: () => {
    accessToken = null;
    localStorage.removeItem(REFRESH_KEY);
  },
};
