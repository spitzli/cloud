export const mailSendExample=`curl https://api.webdock.dev/api/mail/v1/emails \\
  -H "Authorization: Bearer $WEBDOCK_MAIL_API_KEY" \\
  -H "Content-Type: application/json" \\
  -H "Idempotency-Key: order-42-receipt" \\
  -d '{"from":"Shop <mail@example.com>","to":"delivered@test.mail.webdock.dev","subject":"Receipt","text":"Thank you."}'`;
export const mailFetchExample=`const response = await fetch("https://api.webdock.dev/api/mail/v1/emails", {
  method: "POST",
  headers: {
    Authorization: "Bearer " + process.env.WEBDOCK_MAIL_API_KEY,
    "Content-Type": "application/json",
    "Idempotency-Key": "order-42-receipt"
  },
  body: JSON.stringify({from:"mail@example.com",to:"delivered@test.mail.webdock.dev",subject:"Receipt",text:"Thank you."})
});
const result = await response.json();
// If result.name === "send_outcome_unknown", inspect result.email_id. Never resend with a new key.`;
export const mailAdvancedExamples={
 batch:{path:'/emails/batch',method:'POST',body:[{from:'mail@example.com',to:'delivered@test.mail.webdock.dev',subject:'One',text:'First'},{from:'mail@example.com',to:'delivered@test.mail.webdock.dev',subject:'Two',text:'Second'}]},
 attachment:{path:'/emails',method:'POST',body:{from:'mail@example.com',to:'delivered@test.mail.webdock.dev',subject:'Attachment',text:'Attached',attachments:[{filename:'hello.txt',content_type:'text/plain',content:'SGVsbG8='}]}},
 inlineImage:{path:'/emails',method:'POST',body:{from:'mail@example.com',to:'delivered@test.mail.webdock.dev',subject:'CID image',html:'<p>Embedded image</p><img src="cid:logo@example.com" alt="Logo">',attachments:[{filename:'logo.png',content_type:'image/png',content_id:'logo',content:'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jD1sAAAAASUVORK5CYII='}]}},
 headers:{path:'/emails',method:'POST',body:{from:'mail@example.com',to:'delivered@test.mail.webdock.dev',cc:'bounced@test.mail.webdock.dev',bcc:'complained@test.mail.webdock.dev',reply_to:'support@example.com',subject:'Recipient metadata',text:'Simulator only',headers:{'X-Order-ID':'order-42'},tags:[{name:'category',value:'receipt'}]}},
 schedule:{path:'/emails',method:'POST',body:{from:'mail@example.com',to:'delivered@test.mail.webdock.dev',subject:'Scheduled',text:'Later',scheduled_at:'REPLACE_WITH_FUTURE_ISO_TIME_WITH_ZONE'}},
};
export const mailWebhookVerificationExample=`import { createHmac, timingSafeEqual } from "node:crypto";

export async function POST(request) {
  const raw = await request.text(); // Verify before JSON.parse.
  const id = request.headers.get("webhook-id") ?? "";
  const time = request.headers.get("webhook-timestamp") ?? "";
  const signature = request.headers.get("webhook-signature") ?? "";
  if (!/^[0-9]+$/.test(time) || Math.abs(Date.now()/1000-Number(time)) > 300)
    return new Response(null, {status: 401});
  const secret = process.env.WEBDOCK_WEBHOOK_SECRET;
  const expected = "v1," + createHmac("sha256", Buffer.from(secret.slice(6), "base64"))
    .update(id + "." + time + "." + raw).digest("base64");
  const actual = Buffer.from(signature), wanted = Buffer.from(expected);
  if (actual.length !== wanted.length || !timingSafeEqual(actual, wanted))
    return new Response(null, {status: 401});
  const event = JSON.parse(raw);
  // In one DB transaction: insert UNIQUE(webhook-id), then apply your changes.
  // An already recorded id returns 200 without repeating the changes.
  // Return 2xx only after the transaction commits.
  return new Response(null, {status: 200});
}`;
