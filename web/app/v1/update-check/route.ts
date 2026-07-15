import { NextResponse } from 'next/server';
import { promises as fs } from 'fs';
import path from 'path';
import { kv } from '@vercel/kv';

const useKV = !!process.env.KV_REST_API_URL;
const isVercel = process.env.VERCEL === '1';

const logFilePath = isVercel 
    ? path.join('/tmp', 'telemetry_log.json')
    : path.join(process.cwd(), 'telemetry_log.json');

const analyticsFilePath = isVercel
    ? path.join('/tmp', 'telemetry_analytics.json')
    : path.join(process.cwd(), 'telemetry_analytics.json');

async function saveTelemetryLog(data: any) {
    if (useKV) {
        try {
            await kv.lpush('telemetry_logs', JSON.stringify({
                timestamp: new Date().toISOString(),
                ...data
            }));
            await kv.ltrim('telemetry_logs', 0, 999);
        } catch (e) {
            console.error('[KV Save Log Error]', e);
        }
    } else {
        try {
            let logs: any[] = [];
            try {
                const fileData = await fs.readFile(logFilePath, 'utf8');
                logs = JSON.parse(fileData);
            } catch (e) {
                // File does not exist yet
            }
            
            logs.push({
                timestamp: new Date().toISOString(),
                ...data
            });

            if (logs.length > 1000) {
                logs = logs.slice(logs.length - 1000);
            }

            await fs.writeFile(logFilePath, JSON.stringify(logs, null, 2), 'utf8');
        } catch (err) {
            console.error('[Local Logger Error]', err);
        }
    }
}

async function updateAnalytics(type: string, data: any) {
    if (useKV) {
        try {
            if (data.client_hash) {
                await kv.sadd('active_instances', data.client_hash);
            }
            if (type === 'tsum_update') {
                await kv.incrbyfloat('total_spend_under_management', data.cost || 0);
                await kv.incrby('total_requests_managed', 1);
            } else if (type === 'daily_mapd') {
                await kv.incrby('total_tokens_managed', data.total_tokens || 0);
            }
        } catch (e) {
            console.error('[KV Analytics Error]', e);
        }
    } else {
        try {
            let analytics = {
                total_spend_under_management: 0.0,
                total_requests_managed: 0,
                total_tokens_managed: 0,
                active_instances: [] as string[]
            };
            try {
                const fileData = await fs.readFile(analyticsFilePath, 'utf8');
                analytics = JSON.parse(fileData);
            } catch (e) {
                // File does not exist yet
            }

            if (data.client_hash && !analytics.active_instances.includes(data.client_hash)) {
                analytics.active_instances.push(data.client_hash);
            }

            if (type === 'tsum_update') {
                analytics.total_spend_under_management += data.cost || 0;
                analytics.total_requests_managed += 1;
            } else if (type === 'daily_mapd') {
                analytics.total_tokens_managed += data.total_tokens || 0;
            }

            await fs.writeFile(analyticsFilePath, JSON.stringify(analytics, null, 2), 'utf8');
        } catch (e) {
            console.error('[Local Analytics Error]', e);
        }
    }
}

// GET endpoint (for backward compatibility and manual checks)
export async function GET(request: Request) {
    const { searchParams } = new URL(request.url);
    const version = searchParams.get('version') || '0.0.0';
    const isDocker = searchParams.get('is_docker') === 'true';
    const os = searchParams.get('os') || 'unknown';
    const arch = searchParams.get('arch') || 'unknown';
    const ip = "incognito";

    await saveTelemetryLog({ type: 'get_check', version, is_docker: isDocker, os, arch, ip });

    const latestVersion = '1.3.0';
    const updateAvailable = version !== latestVersion;

    return NextResponse.json({
        latest_version: latestVersion,
        update_available: updateAvailable,
        message: "GET update-check successful."
    });
}

// POST endpoint (receives structured telemetry payloads: startup, daily_mapd, tsum_update)
export async function POST(request: Request) {
    try {
        const payload = await request.json();
        const type = payload.type || 'unknown';
        const clientHash = payload.client_hash || 'unknown';
        const ip = "incognito";

        // 1. Process and save log entries based on types
        if (type === 'startup') {
            await saveTelemetryLog({
                type: 'startup',
                client_hash: clientHash,
                version: payload.version || '0.0.0',
                isDocker: payload.is_docker === true,
                os: payload.os || 'unknown',
                arch: payload.arch || 'unknown',
                ip
            });
            await updateAnalytics('startup', { client_hash: clientHash });
        } else if (type === 'daily_mapd') {
            await saveTelemetryLog({
                type: 'daily_mapd',
                client_hash: clientHash,
                version: payload.version || '0.0.0',
                total_requests: payload.total_requests || 0,
                total_tokens: payload.total_tokens || 0,
                total_users: payload.total_users || 0,
                model_distribution: payload.model_distribution || {},
                ip
            });
            await updateAnalytics('daily_mapd', { client_hash: clientHash, total_tokens: payload.total_tokens });
        } else if (type === 'tsum_update') {
            await updateAnalytics('tsum_update', { client_hash: clientHash, cost: payload.cost });
            return NextResponse.json({ success: true });
        }

        const latestVersion = '1.3.0';
        const currentVersion = payload.version || '0.0.0';
        const updateAvailable = currentVersion !== latestVersion;

        return NextResponse.json({
            latest_version: latestVersion,
            update_available: updateAvailable,
            message: `Telemetry payload type '${type}' ingested successfully.`
        });
    } catch (e) {
        return NextResponse.json({ error: 'Failed to process telemetry payload' }, { status: 400 });
    }
}
