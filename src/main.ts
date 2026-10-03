import { bootstrap } from "./app/bootstrap";

performance.mark("snapclip-main-loaded");
console.info("[snapclip][frontend] main module loaded");
bootstrap();
