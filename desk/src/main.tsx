import "./design/tokens.css";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { applyRememberedAppearance } from "./app/appearance";
import { RolleProvider } from "./app/rollen";
import { router } from "./app/router";
import { applyRememberedSprache } from "./i18n";

applyRememberedAppearance();

const queries = new QueryClient({
  defaultOptions: {
    queries: { retry: false, staleTime: 10_000, refetchOnWindowFocus: true },
  },
});

const root = document.getElementById("root");
if (!root) throw new Error("index.html has no #root");

const mount = root;
// An English Desk waits for its catalogue before the first paint; a German one does not wait.
void applyRememberedSprache().then(() =>
  createRoot(mount).render(
    <StrictMode>
      <QueryClientProvider client={queries}>
        <RolleProvider>
          <RouterProvider router={router} />
        </RolleProvider>
      </QueryClientProvider>
    </StrictMode>,
  ),
);
