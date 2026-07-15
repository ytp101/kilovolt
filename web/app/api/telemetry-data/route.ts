import { NextResponse } from 'next/server';
import { promises as fs } from 'fs';
import path from 'path';
import { cookies } from 'next/headers';
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

export async function GET() {
    // 1. Session verification
    const cookieStore = await cookies();
    const session = cookieStore.get('admin_session')?.value;

    if (session !== 'authenticated') {
        return NextResponse.json({ error: 'Unauthorized access' }, { status: 401 });
    }

    let logs: any[] = [];
    let analytics = {
        total_spend_under_management: 0.0,
        total_requests_managed: 0,
        total_tokens_managed: 0,
        active_instances: [] as string[]
    };

    // 2. Fetch metrics from either Supabase or local files
    if (useSupabase && supabase) {
        try {
            // Retrieve last 100 logs
            const { data: dbLogs } = await supabase
                .from('telemetry_logs')
                .select('*')
                .order('timestamp', { ascending: false })
                .limit(100);

            logs = (dbLogs || []).map((l: any) => ({
                timestamp: l.timestamp,
                type: l.type,
                client_hash: l.client_hash,
                version: l.version,
                isDocker: l.is_docker,
                os: l.os,
                arch: l.arch,
                ip: l.ip,
                total_requests: l.total_requests,
                total_tokens: l.total_tokens,
                total_users: l.total_users,
                model_distribution: l.model_distribution
            })).reverse(); // Reverse back to chronological order for stats processing in client

            // Calculate aggregations dynamically
            // A. TSUM (Sum of cost from tsum_updates)
            const { data: costData } = await supabase
                .from('telemetry_logs')
                .select('cost')
                .eq('type', 'tsum_update');
            const tsum = (costData || []).reduce((acc: number, item: any) => acc + (item.cost || 0), 0);

            // B. Total Requests (Count of tsum_updates)
            const { count: requestsCount } = await supabase
                .from('telemetry_logs')
                .select('*', { count: 'exact', head: true })
                .eq('type', 'tsum_update');
            const reqs = requestsCount || 0;

            // C. Total Tokens Transited (Sum of total_tokens from daily_mapds)
            const { data: tokensData } = await supabase
                .from('telemetry_logs')
                .select('total_tokens')
                .eq('type', 'daily_mapd');
            const tokens = (tokensData || []).reduce((acc: number, item: any) => acc + (item.total_tokens || 0), 0);

            // D. Active Instances (Deduplicated Client Hashes)
            const { data: instancesData } = await supabase
                .from('telemetry_logs')
                .select('client_hash');
            const instancesSet = new Set((instancesData || []).map((item: any) => item.client_hash).filter(Boolean));
            const instances = Array.from(instancesSet);

            analytics = {
                total_spend_under_management: tsum,
                total_requests_managed: reqs,
                total_tokens_managed: tokens,
                active_instances: instances
            };
        } catch (e) {
            console.error('[Supabase Query Error]', e);
        }
    } else {
        try {
            const fileData = await fs.readFile(logFilePath, 'utf8');
            logs = JSON.parse(fileData);
        } catch (e) {
            // Logs remain empty
        }

        try {
            const fileData = await fs.readFile(analyticsFilePath, 'utf8');
            analytics = JSON.parse(fileData);
        } catch (e) {
            // Analytics remain defaults
        }
    }

    const storageType = useSupabase 
        ? 'Supabase Cloud (Persistent Postgres)' 
        : (isVercel ? 'Vercel Serverless /tmp (Ephemeral)' : 'Local Disk (Persistent)');

    return NextResponse.json({ logs, analytics, storage_type: storageType });
}
