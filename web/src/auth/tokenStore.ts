let accessToken: string | null = null;

export const tokenStore = {
  getAccessToken: () => accessToken,
  setAccessToken: (t: string | null) => { accessToken = t; },
  clear: () => { accessToken = null; },
};
