// Entry point: wire the routes onto a Fastify instance and listen.
import Fastify from "fastify";
import { registerRoutes } from "./routes.js";

const app = Fastify({ logger: true });
registerRoutes(app);

const port = Number(process.env.PORT ?? 3000);
app.listen({ port }).catch((err) => {
  app.log.error(err);
  process.exit(1);
});
