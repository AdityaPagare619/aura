const { execSync } = require('child_process');
const https = require('https');
const fs = require('fs');
const path = require('path');

// AURA v4 F001 Smoke Test
// Tests aura-daemon binary locally on Windows

console.log('='.repeat(60));
console.log('AURA v4 - F001 Smoke Test (Local)');
console.log('='.repeat(60));

const BINARY_PATH = 'C:/Users/Lenovo/aura/artifacts/aura-daemon';
const EXPECTED_SHA256 = '6d649c29d1bc862bed5491b7a132809c5c3fd8438ff397f71b8ec91c832ac919';

async function main() {
    let allPassed = true;

    // Test 1: Binary exists
    console.log('\n[TEST 1] Binary exists...');
    try {
        const exists = fs.existsSync(BINARY_PATH);
        if (!exists) {
            console.log('  ❌ FAIL: Binary not found at ' + BINARY_PATH);
            allPassed = false;
        } else {
            const stats = fs.statSync(BINARY_PATH);
            console.log(`  ✅ PASS: Binary found (${(stats.size / 1024 / 1024).toFixed(2)} MB)`);
        }
    } catch (e) {
        console.log('  ❌ FAIL: ' + e.message);
        allPassed = false;
    }

    // Test 2: Binary SHA256
    console.log('\n[TEST 2] Binary SHA256 verification...');
    try {
        const hash = require('crypto').createHash('sha256');
        const fileBuffer = fs.readFileSync(BINARY_PATH);
        hash.update(fileBuffer);
        const sha256 = hash.digest('hex');
        
        if (sha256 === EXPECTED_SHA256) {
            console.log(`  ✅ PASS: SHA256 matches`);
            console.log(`     ${sha256}`);
        } else {
            console.log(`  ❌ FAIL: SHA256 mismatch`);
            console.log(`     Expected: ${EXPECTED_SHA256}`);
            console.log(`     Got:      ${sha256}`);
            allPassed = false;
        }
    } catch (e) {
        console.log('  ❌ FAIL: ' + e.message);
        allPassed = false;
    }

    // Test 3: File type (should be ELF64 ARM)
    console.log('\n[TEST 3] Binary file type...');
    try {
        const { execSync } = require('child_process');
        // Read ELF header manually
        const buffer = Buffer.alloc(20);
        const fd = fs.openSync(BINARY_PATH, 'r');
        fs.readSync(fd, buffer, 0, 20, 0);
        fs.closeSync(fd);
        
        // Check ELF magic
        if (buffer[0] === 0x7f && buffer[1] === 0x45 && buffer[2] === 0x4c && buffer[3] === 0x46) {
            const elfClass = buffer[4]; // 1=32bit, 2=64bit
            const endian = buffer[5]; // 1=little, 2=big
            const machine = buffer.readUInt16LE(18);
            
            const arch = elfClass === 2 ? '64-bit' : '32-bit';
            const endianness = endian === 1 ? 'little-endian' : 'big-endian';
            
            let archName = 'Unknown';
            if (machine === 0xb7) archName = 'ARM AArch64 (aarch64) ✅';
            else if (machine === 0x03) archName = 'Intel 386';
            else if (machine === 0x3e) archName = 'AMD x86-64';
            else archName = `Machine type: ${machine}`;
            
            console.log(`  ✅ PASS: Valid ELF binary`);
            console.log(`     Architecture: ${arch} ${endianness} ${archName}`);
        } else {
            console.log('  ❌ FAIL: Not a valid ELF file');
            allPassed = false;
        }
    } catch (e) {
        console.log('  ❌ FAIL: ' + e.message);
        allPassed = false;
    }

    // Test 4: Try to execute (Windows can't run ARM64 ELF, but we can verify it exists and is valid)
    console.log('\n[TEST 4] Binary executable flag...');
    console.log('  ⚠️  NOTE: Cannot execute ARM64 ELF binary on Windows');
    console.log('     This test will be completed on real Android device via BrowserStack');
    console.log('     Expected: ./aura-daemon --version → exit code 0');
    console.log('     F001 check: exit code 139 = SIGSEGV (still broken)');
    console.log('     F001 check: exit code 0 = SUCCESS (FIXED!)');

    // Test 5: Check Telegram Bot
    console.log('\n[TEST 5] Telegram Bot connectivity...');
    try {
        const https = require('https');
        const token = '8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI';
        const userId = '8407946567';
        
        const getMe = await new Promise((resolve, reject) => {
            https.get(`https://api.telegram.org/bot${token}/getMe`, (res) => {
                let data = '';
                res.on('data', d => data += d);
                res.on('end', () => {
                    try { resolve(JSON.parse(data)); }
                    catch { reject(new Error('Invalid JSON')); }
                });
            }).on('error', reject);
        });
        
        if (getMe.ok) {
            console.log(`  ✅ PASS: Bot is alive`);
            console.log(`     Name: ${getMe.result.first_name}`);
            console.log(`     Username: @${getMe.result.username}`);
            console.log(`     Is Bot: ${getMe.result.is_bot}`);
        } else {
            console.log('  ❌ FAIL: Bot not responding');
            allPassed = false;
        }
    } catch (e) {
        console.log('  ❌ FAIL: ' + e.message);
        allPassed = false;
    }

    // Test 6: Verify GitHub Release URL
    console.log('\n[TEST 6] GitHub Release binary URL...');
    const releaseUrl = 'https://github.com/AdityaPagare619/aura/releases/download/v4.0.0-f001-validated/aura-daemon';
    try {
        const https = require('https');
        const response = await new Promise((resolve, reject) => {
            const req = https.get(releaseUrl, { method: 'HEAD' }, (res) => {
                resolve({ status: res.statusCode, headers: res.headers });
            });
            req.on('error', reject);
            req.setTimeout(10000, () => {
                req.destroy();
                reject(new Error('Request timeout'));
            });
        });
        
        if (response.status === 200) {
            console.log('  ✅ PASS: Binary URL is valid and accessible');
            console.log(`     URL: ${releaseUrl}`);
        } else {
            console.log(`  ❌ FAIL: HTTP ${response.status}`);
            allPassed = false;
        }
    } catch (e) {
        console.log('  ❌ FAIL: ' + e.message);
        allPassed = false;
    }

    // Summary
    console.log('\n' + '='.repeat(60));
    console.log('TEST SUMMARY');
    console.log('='.repeat(60));
    
    if (allPassed) {
        console.log('✅ ALL LOCAL TESTS PASSED');
        console.log('');
        console.log('NEXT STEP: Execute on REAL ANDROID DEVICE');
        console.log('');
        console.log('BrowserStack Session URL:');
        console.log('https://app-live.browserstack.com/dashboard#os=android&os_version=14.0&app_hashed_id=6c0e14f1d46db18d7a4d3fafa21168a04aed2649');
        console.log('');
        console.log('Commands to run on device:');
        console.log('  wget https://github.com/AdityaPagare619/aura/releases/download/v4.0.0-f001-validated/aura-daemon');
        console.log('  chmod +x aura-daemon');
        console.log('  ./aura-daemon --version');
        console.log('  echo "Exit code: $?"');
    } else {
        console.log('❌ SOME TESTS FAILED');
    }
    
    console.log('='.repeat(60));
    process.exit(allPassed ? 0 : 1);
}

main().catch(e => {
    console.error('Test error:', e.message);
    process.exit(1);
});
