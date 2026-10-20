'use strict';
const {chromium} = require(process.env.NEEDLE_PLAYWRIGHT);
const fs = require('fs');
(async () => {
  const browser = await chromium.launch({executablePath:'/usr/bin/chromium',headless:true,args:['--no-sandbox']});
  try {
    // Test-only CA is not installed in the machine trust store. TLS was independently
    // verified by the Rust agent and Python HTTPS client before this browser pass.
    const context = await browser.newContext({ignoreHTTPSErrors:true,viewport:{width:1440,height:1050},deviceScaleFactor:1});
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror',error => errors.push(error.message));
    await page.goto(process.env.NEEDLE_TEST_ORIGIN);
    await page.locator('#token').fill(process.env.NEEDLE_TEST_TOKEN);
    await page.locator('button[type=submit]').click();
    await page.locator('#dashboard').waitFor({state:'visible'});
    if (await page.locator('.node-name').textContent() !== 'Local · 实机采样') throw new Error('real node not rendered');
    const leak = await page.evaluate(token => Object.values(localStorage).includes(token) || Object.values(sessionStorage).includes(token),process.env.NEEDLE_TEST_TOKEN);
    if(leak) throw new Error('credential persisted in browser storage');
    const desktopOverflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth);
    if(desktopOverflow) throw new Error('desktop overflow');
    await page.screenshot({path:process.env.NEEDLE_TEST_WORK+'/desktop.png',fullPage:true});
    await page.locator('#theme').click();
    await page.screenshot({path:process.env.NEEDLE_TEST_WORK+'/dark.png',fullPage:true});
    await page.setViewportSize({width:390,height:844});
    if(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)) throw new Error('mobile overflow');
    await page.screenshot({path:process.env.NEEDLE_TEST_WORK+'/mobile.png',fullPage:true});
    await page.locator('#logout').click();
    await page.locator('#login').waitFor({state:'visible'});
    if(await page.locator('.node').count()) throw new Error('lock did not clear metrics');
    if(errors.length) throw new Error(errors.join('\n'));
    fs.writeFileSync(process.env.NEEDLE_TEST_WORK+'/browser.json',JSON.stringify({pageErrors:errors,desktopOverflow:false,mobileOverflow:false,credentialPersisted:false,lockClearsData:true},null,2));
    console.log('PASS Chromium desktop/mobile real-data rendering, theme switch, no overflow, no credential storage, lock clears data');
  } finally { await browser.close(); }
})().catch(e => {console.error(e);process.exit(1)});
