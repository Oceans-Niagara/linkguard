export interface Env {
  SIGNING_KEY_B64: string;       // secret: contents of linkguard-private.key
  STRIPE_WEBHOOK_SECRET: string; // secret: whsec_...
  STRIPE_API_KEY?: string;       // secret: rk_live_... or sk_live_... (needed for Price ID lookup & /claim)
  RESEND_API_KEY?: string;       // secret: re_... from Resend
  FROM_EMAIL: string;
  SUPPORT_EMAIL: string;
  PRO_PRICE_ID: string;
  TEAM_PRICE_ID: string;
  ALLOWED_ORIGIN: string;
}

const PKCS8_ED25519_PREFIX = new Uint8Array([
  0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06,
  0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
]);

function toBase64Url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function fromBase64Url(s: string): Uint8Array {
  const clean = s.trim();
  const b64 = clean.replace(/-/g, "+").replace(/_/g, "/") +
    "=".repeat((4 - (clean.length % 4)) % 4);
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function timingSafeEqualHex(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) {
    diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  }
  return diff === 0;
}

async function verifyStripeSignature(
  rawBody: string,
  sigHeader: string | null,
  secret: string,
): Promise<boolean> {
  if (!sigHeader) return false;
  const parts = Object.fromEntries(
    sigHeader.split(",").map((kv) => kv.split("=") as [string, string]),
  );
  const timestamp = parts["t"];
  const v1 = parts["v1"];
  if (!timestamp || !v1) return false;

  const age = Math.abs(Math.floor(Date.now() / 1000) - Number(timestamp));
  if (!Number.isFinite(age) || age > 300) return false;

  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const mac = await crypto.subtle.sign(
    "HMAC",
    key,
    new TextEncoder().encode(`${timestamp}.${rawBody}`),
  );
  const expectedHex = [...new Uint8Array(mac)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");

  return timingSafeEqualHex(expectedHex, v1);
}

async function mintLicenseToken(
  seedB64: string,
  tier: "pro" | "team",
  holder: string,
  licenseId: string,
  issuedUnix: number,
): Promise<string> {
  const seed = fromBase64Url(seedB64);
  if (seed.length !== 32) throw new Error("SIGNING_KEY_B64 must be 32 bytes");

  const pkcs8 = new Uint8Array(PKCS8_ED25519_PREFIX.length + 32);
  pkcs8.set(PKCS8_ED25519_PREFIX, 0);
  pkcs8.set(seed, PKCS8_ED25519_PREFIX.length);

  const privateKey = await crypto.subtle.importKey(
    "pkcs8",
    pkcs8,
    { name: "Ed25519" },
    false,
    ["sign"],
  );

  const claims = {
    tier,
    holder,
    issued_unix: issuedUnix,
    expires_unix: null, // Lifetime v0.3 license
    license_id: licenseId,
  };

  const payloadB64 = toBase64Url(new TextEncoder().encode(JSON.stringify(claims)));
  const msg = `LG1.${payloadB64}`;
  const sig = new Uint8Array(
    await crypto.subtle.sign("Ed25519", privateKey, new TextEncoder().encode(msg)),
  );
  return `${msg}.${toBase64Url(sig)}`;
}

async function fetchStripeSessionWithItems(
  sessionId: string,
  apiKey: string,
): Promise<any> {
  const url = `https://api.stripe.com/v1/checkout/sessions/${encodeURIComponent(sessionId)}?expand[]=line_items`;
  const resp = await fetch(url, {
    headers: { Authorization: `Bearer ${apiKey}` },
  });
  if (!resp.ok) {
    throw new Error(`Stripe API error (${resp.status})`);
  }
  return resp.json();
}

function resolveTierFromSession(session: any, env: Env): "pro" | "team" | null {
  const metaTier = String(session.metadata?.tier || "").toLowerCase();
  if (metaTier === "team") return "team";
  if (metaTier === "pro") return "pro";

  const items: any[] = session.line_items?.data ?? [];
  for (const item of items) {
    const priceId = item.price?.id;
    if (priceId === env.TEAM_PRICE_ID) return "team";
    if (priceId === env.PRO_PRICE_ID) return "pro";
  }
  return null;
}

async function sendLicenseEmail(
  env: Env,
  toEmail: string,
  tier: "pro" | "team",
  token: string,
): Promise<void> {
  if (!env.RESEND_API_KEY) return;
  const resp = await fetch("https://api.resend.com/emails", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${env.RESEND_API_KEY}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      from: env.FROM_EMAIL,
      reply_to: env.SUPPORT_EMAIL,
      to: [toEmail],
      subject: `Your LinkGuard ${tier.toUpperCase()} (v0.3) Lifetime Offline License`,
      text:
        `Thanks for purchasing LinkGuard ${tier.toUpperCase()}!\n\n` +
        `Your offline Ed25519 license key (v0.3 lifetime):\n\n` +
        `${token}\n\n` +
        `How to activate (100% offline, zero network calls):\n` +
        `  Option A: Save the key above to a file named linkguard.lic in your project folder\n` +
        `  Option B: Set environment variable LINKGUARD_LICENSE="<key>"\n\n` +
        `Verify anytime:\n` +
        `  linkguard license\n` +
        `  linkguard gui --profile lora-sf12\n\n` +
        `Questions? Reply to ${env.SUPPORT_EMAIL}.\n`,
    }),
  });
  if (!resp.ok) {
    console.error("Resend email failed:", resp.status, await resp.text());
  }
}

function corsHeaders(env: Env, request?: Request): Record<string, string> {
  const allowed = env.ALLOWED_ORIGIN.split(",").map((s) => s.trim()).filter(Boolean);
  const reqOrigin = request?.headers.get("Origin")?.trim();
  const origin = reqOrigin && allowed.includes(reqOrigin) ? reqOrigin : allowed[0] || "*";
  return {
    "Access-Control-Allow-Origin": origin,
    "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
    "Access-Control-Allow-Headers": "Content-Type",
    "Vary": "Origin",
  };
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);

    if (request.method === "OPTIONS") {
      return new Response(null, { status: 204, headers: corsHeaders(env, request) });
    }

    // Instant post-checkout key claim for your GitHub Pages success page:
    // GET /claim?session_id=cs_...
    if (request.method === "GET" && url.pathname === "/claim") {
      const sessionId = url.searchParams.get("session_id")?.trim();
      if (!sessionId || !sessionId.startsWith("cs_")) {
        return new Response(JSON.stringify({ error: "Invalid session_id" }), {
          status: 400,
          headers: { "Content-Type": "application/json", ...corsHeaders(env, request) },
        });
      }
      if (!env.STRIPE_API_KEY) {
        return new Response(JSON.stringify({ error: "Claim endpoint not configured" }), {
          status: 501,
          headers: { "Content-Type": "application/json", ...corsHeaders(env, request) },
        });
      }

      try {
        const session = await fetchStripeSessionWithItems(sessionId, env.STRIPE_API_KEY);
        if (session.payment_status !== "paid") {
          return new Response(JSON.stringify({ error: "Payment not completed yet" }), {
            status: 402,
            headers: { "Content-Type": "application/json", ...corsHeaders(env, request) },
          });
        }

        const tier = resolveTierFromSession(session, env) ?? "pro";
        const email: string =
          session.customer_details?.email || session.customer_email || "customer";
        const issuedUnix = Number(session.created) || Math.floor(Date.now() / 1000);

        const token = await mintLicenseToken(
          env.SIGNING_KEY_B64,
          tier,
          email,
          session.id,
          issuedUnix,
        );

        return new Response(JSON.stringify({ tier, holder: email, license: token }), {
          status: 200,
          headers: { "Content-Type": "application/json", ...corsHeaders(env, request) },
        });
      } catch (e: any) {
        return new Response(JSON.stringify({ error: e?.message || "Lookup failed" }), {
          status: 500,
          headers: { "Content-Type": "application/json", ...corsHeaders(env, request) },
        });
      }
    }

    // Stripe Webhook: POST / or POST /webhook
    if (request.method !== "POST") {
      return new Response("Method Not Allowed", { status: 405 });
    }

    const rawBody = await request.text();
    const ok = await verifyStripeSignature(
      rawBody,
      request.headers.get("stripe-signature"),
      env.STRIPE_WEBHOOK_SECRET,
    );
    if (!ok) return new Response("Invalid signature", { status: 400 });

    const event = JSON.parse(rawBody);
    if (event.type !== "checkout.session.completed") {
      return new Response("Ignored event", { status: 200 });
    }

    let session = event.data.object;
    if (session.payment_status !== "paid") {
      return new Response("Payment pending", { status: 200 });
    }

    let tier = resolveTierFromSession(session, env);
    if (!tier && env.STRIPE_API_KEY) {
      session = await fetchStripeSessionWithItems(session.id, env.STRIPE_API_KEY);
      tier = resolveTierFromSession(session, env);
    }
    if (!tier) {
      return new Response("Unrecognized product price ID", { status: 400 });
    }

    const email: string =
      session.customer_details?.email || session.customer_email || "customer";
    const issuedUnix = Number(session.created) || Math.floor(Date.now() / 1000);

    const token = await mintLicenseToken(
      env.SIGNING_KEY_B64,
      tier,
      email,
      session.id,
      issuedUnix,
    );

    await sendLicenseEmail(env, email, tier, token);

    return new Response(JSON.stringify({ issued: true, tier, license_id: session.id }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  },
};