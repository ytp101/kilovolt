import { NextResponse } from 'next/server';
import { promises as fs } from 'fs';
import path from 'path';
import { createClient } from '@supabase/supabase-js';

const supabaseUrl = process.env.SUPABASE_URL || '';
const supabaseKey = process.env.SUPABASE_SERVICE_ROLE_KEY || process.env.SUPABASE_ANON_KEY || '';

const useSupabase = !!(supabaseUrl && supabaseKey);
const supabase = useSupabase ? createClient(supabaseUrl, supabaseKey) : null;

const isVercel = process.env.VERCEL === '1';

const logFilePath = isVercel 
    ? path.join('/tmp', 'telemetry_log.json')
    : path.join(process.cwd(), 'telemetry_log.json');

const analyticsFilePath = isVercel
    ? path.join('/tmp', 'telemetry_analytics.json')
    : path.join(process.cwd(), 'telemetry_analytics.json');

async function saveTelemetryLog(data: any) {
    if (useSupabase && supabase) {
        try {
            await supabase.from('telemetry_logs').insert({
                type: data.type,
                client_hash: data.client_hash || null,
                version: data.version || null,
                is_docker: data.is_docker !== undefined ? data.is_docker : null,
                os: data.os || null,
                arch: data.arch || null,
                ip: data.ip || 'incognito',
                cost: data.cost !== undefined ? data.cost : null,
                total_requests: data.total_requests !== undefined ? data.total_requests : null,
                total_tokens: data.total_tokens !== undefined ? data.total_tokens : null,
                total_users: data.total_users !== undefined ? data.total_users : null,
                model_distribution: data.model_distribution || null
            });
        } catch (e) {
            console.error('[Supabase Save Log Error]', e);
        }
    } else {
        try {
            // Local file logger fallback
            let logs: any[] = [];
            try {
                const fileData = await fs.readFile(logFilePath, 'utf8');
                logs = JSON.parse(fileData);
            } catch (e) {}

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

async function updateAnalyticsFallback(type: string, data: any) {
    if (useSupabase) {
        // SQL aggregates will compute analytics on the fly, no need to update secondary tables
        return;
    }

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
        } catch (e) {}

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

        // Process and save log entries based on types
        if (type === 'startup') {
            await saveTelemetryLog({
                type: 'startup',
                client_hash: clientHash,
                version: payload.version || '0.0.0',
                is_docker: payload.is_docker === true,
                os: payload.os || 'unknown',
                arch: payload.arch || 'unknown',
                ip
            });
            await updateAnalyticsFallback('startup', { client_hash: clientHash });
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
            await updateAnalyticsFallback('daily_mapd', { client_hash: clientHash, total_tokens: payload.total_tokens });
        } else if (type === 'tsum_update') {
            await saveTelemetryLog({
                type: 'tsum_update',
                client_hash: clientHash,
                cost: payload.cost || 0,
                ip
            });
            await updateAnalyticsFallback('tsum_update', { client_hash: clientHash, cost: payload.cost });
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
