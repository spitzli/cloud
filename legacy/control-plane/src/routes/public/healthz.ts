import Elysia from "elysia";
export default new Elysia().get("", () => ({ status: "ok" }));
