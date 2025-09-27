import init, { FeeExplorer } from './pkg/fee_explorer.js';

class BitcoinFeeExplorer {
    constructor() {
        this.wasmModule = null;
        this.explorer = null;
        this.metadata = null;
        this.selectedMetrics = new Set();
        this.chart = null;
    }

    async initialize() {
        try {
            this.updateStatus('Loading WASM module...');

            // Initialize WASM
            await init();
            this.explorer = new FeeExplorer();

            this.updateStatus('Loading metadata...');
            await this.loadMetadata();

            this.updateStatus('Loading datasets...');
            await this.loadDatasets();

            this.setupUI();
            this.updateStatus('Ready! Select metrics and update chart.');

        } catch (error) {
            console.error('Initialization failed:', error);
            this.updateStatus(`Error: ${error.message}`);
        }
    }

    async loadMetadata() {
        const response = await fetch('./data/datasets.json');
        if (!response.ok) {
            throw new Error(`Failed to load metadata: ${response.statusText}`);
        }

        const metadataText = await response.text();
        this.metadata = JSON.parse(metadataText);

        // Don't load into WASM yet - we'll update the range first after loading Arrow files
    }

    async loadDatasets() {
        // Load real Arrow files
        await this.loadArrowDatasets();
    }

    async loadArrowDatasets() {
        console.log('Loading real Arrow datasets...');

        // Check if Arrow library is loaded
        if (typeof Arrow === 'undefined') {
            throw new Error('Apache Arrow library not loaded. Check if the CDN is accessible.');
        }

        console.log('Arrow library available:', typeof Arrow);
        this.arrowData = new Map();
        let minHeight = Infinity;
        let maxHeight = -Infinity;

        // Load each Arrow file from the metadata
        for (const dataset of this.metadata.datasets) {
            try {
                console.log(`Loading ${dataset.file}...`);

                const response = await fetch(`./data/${dataset.file}`);
                console.log(`Response status for ${dataset.file}:`, response.status, response.statusText);

                if (!response.ok) {
                    throw new Error(`Failed to load ${dataset.file}: ${response.status} ${response.statusText}`);
                }

                const arrayBuffer = await response.arrayBuffer();
                console.log(`${dataset.file} size:`, arrayBuffer.byteLength, 'bytes');

                // Try different Arrow API methods depending on version
                let table;
                try {
                    console.log(`Attempting to parse ${dataset.file} with tableFromIPC...`);
                    // Try the newer API first
                    table = Arrow.tableFromIPC(arrayBuffer);
                    console.log(`Successfully parsed ${dataset.file} with tableFromIPC`);
                } catch (e1) {
                    console.log(`tableFromIPC failed for ${dataset.file}:`, e1.message);
                    try {
                        console.log(`Attempting ${dataset.file} with Table.from...`);
                        // Try alternative API
                        table = Arrow.Table.from([Arrow.RecordBatch.from(arrayBuffer)]);
                        console.log(`Successfully parsed ${dataset.file} with Table.from`);
                    } catch (e2) {
                        console.log(`Table.from failed for ${dataset.file}:`, e2.message);
                        try {
                            console.log(`Attempting ${dataset.file} with RecordBatchFileReader...`);
                            // Try reading as IPC file
                            const reader = Arrow.RecordBatchFileReader.from(arrayBuffer);
                            table = new Arrow.Table(reader.readAll());
                            console.log(`Successfully parsed ${dataset.file} with RecordBatchFileReader`);
                        } catch (e3) {
                            console.log('Arrow API attempts failed for', dataset.file, ':', {e1: e1.message, e2: e2.message, e3: e3.message});
                            console.log('Available Arrow methods:', Object.keys(Arrow));
                            throw new Error(`Unable to parse ${dataset.file} with any known API method`);
                        }
                    }
                }

                console.log(`Loaded ${dataset.file}: ${table.numRows} rows, ${table.numCols} columns`);
                console.log('Columns:', table.schema.fields.map(f => f.name));

                // Find min/max heights from this dataset
                try {
                    const heightColumn = table.getChild('height');
                    if (heightColumn && table.numRows > 0) {
                        console.log(`Getting height data from ${dataset.file}...`);
                        const heights = heightColumn.toArray();
                        console.log(`Height array length: ${heights.length}, first few values:`, heights.slice(0, 5));

                        // Use regular loop instead of spread operator to avoid "too many arguments" error
                        let datasetMin = heights[0];
                        let datasetMax = heights[0];
                        for (let i = 1; i < heights.length; i++) {
                            if (heights[i] < datasetMin) datasetMin = heights[i];
                            if (heights[i] > datasetMax) datasetMax = heights[i];
                        }

                        minHeight = Math.min(minHeight, datasetMin);
                        maxHeight = Math.max(maxHeight, datasetMax);
                        console.log(`Dataset ${dataset.file} height range: ${datasetMin} to ${datasetMax}`);
                    }
                } catch (heightError) {
                    console.error(`Error processing height data for ${dataset.file}:`, heightError);
                    throw heightError;
                }

                this.arrowData.set(dataset.name, {
                    table: table,
                    dataset: dataset
                });

            } catch (error) {
                console.error(`Failed to load ${dataset.file}:`, error);
                throw new Error(`Could not load ${dataset.file}. Make sure you've exported the Arrow file first.`);
            }
        }

        // Update metadata with discovered block range
        if (minHeight !== Infinity && maxHeight !== -Infinity) {
            this.metadata.block_range = {
                start: minHeight,
                end: maxHeight
            };
            console.log(`Discovered block range: ${minHeight} to ${maxHeight}`);
        } else {
            console.warn('No height data found in Arrow files, keeping default range');
        }

        // Now load the updated metadata into WASM
        const updatedMetadata = JSON.stringify(this.metadata);
        await this.explorer.load_metadata(updatedMetadata);

        this.updateDataStatus(`Loaded ${this.arrowData.size} Arrow datasets successfully`);
    }

    setupUI() {
        this.populateMetricSelect();
        this.setupEventListeners();
        this.initializeChart();

        // Set default range
        const range = this.metadata.block_range;
        document.getElementById('startHeight').value = range.start;
        document.getElementById('endHeight').value = range.end;
    }

    populateMetricSelect() {
        const select = document.getElementById('metricSelect');
        select.innerHTML = '';

        // Get available metrics from WASM module
        const metrics = this.explorer.get_available_metrics();

        metrics.forEach(metric => {
            const option = document.createElement('option');
            option.value = metric.name;
            option.textContent = `${metric.name} (${metric.unit}) - ${metric.description}`;
            option.dataset.unit = metric.unit;
            option.dataset.dataset = metric.dataset;
            select.appendChild(option);
        });
    }

    setupEventListeners() {
        const metricSelect = document.getElementById('metricSelect');
        const updateButton = document.getElementById('updateChart');
        const resetZoomButton = document.getElementById('resetZoom');
        const logScaleCheckbox = document.getElementById('logScale');
        const showMACheckbox = document.getElementById('showMA');

        metricSelect.addEventListener('change', (e) => {
            this.selectedMetrics.clear();
            Array.from(e.target.selectedOptions).forEach(option => {
                this.selectedMetrics.add({
                    name: option.value,
                    unit: option.dataset.unit,
                    dataset: option.dataset.dataset
                });
            });
        });

        updateButton.addEventListener('click', () => this.updateChart());
        resetZoomButton.addEventListener('click', () => this.resetZoom());

        logScaleCheckbox.addEventListener('change', () => this.updateChartLayout());
        showMACheckbox.addEventListener('change', () => this.updateChart());

        // Enable multi-select with Ctrl/Cmd
        metricSelect.addEventListener('mousedown', (e) => {
            if (e.ctrlKey || e.metaKey) {
                e.preventDefault();
                const option = e.target;
                if (option.tagName === 'OPTION') {
                    option.selected = !option.selected;
                    metricSelect.dispatchEvent(new Event('change'));
                }
            }
        });
    }

    initializeChart() {
        const chartDiv = document.getElementById('mainChart');

        const layout = {
            title: 'BlockStats Explorer',
            height: 600,
            xaxis: {
                title: 'Block Height',
                type: 'linear'
            },
            yaxis: {
                title: 'Left Axis',
                side: 'left',
                overlaying: false
            },
            yaxis2: {
                title: 'Right Axis',
                side: 'right',
                overlaying: 'y'
            },
            legend: {
                x: 0,
                y: 1,
                bgcolor: 'rgba(255,255,255,0.8)'
            },
            hovermode: 'x unified',
            dragmode: 'zoom'
        };

        const config = {
            responsive: true,
            displayModeBar: true,
            modeBarButtonsToAdd: ['pan2d', 'zoom2d', 'zoomIn2d', 'zoomOut2d', 'resetScale2d'],
            scrollZoom: true,
            doubleClick: 'reset+autosize'
        };

        Plotly.newPlot(chartDiv, [], layout, config);
        this.chart = chartDiv;
    }

    async updateChart() {
        if (this.selectedMetrics.size === 0) {
            alert('Please select at least one metric');
            return;
        }

        this.updateStatus('Updating chart...');

        try {
            const startHeight = parseInt(document.getElementById('startHeight').value) || this.metadata.block_range.start;
            const endHeight = parseInt(document.getElementById('endHeight').value) || this.metadata.block_range.end;
            const showMA = document.getElementById('showMA').checked;
            const maWindow = parseInt(document.getElementById('maWindow').value) || 200;

            const traces = [];
            const metricNames = Array.from(this.selectedMetrics).map(m => m.name);

            // Get metric data from Arrow files
            this.createArrowTraces(traces, metricNames, startHeight, endHeight, showMA, maWindow);

            await Plotly.react(this.chart, traces, this.getChartLayout());
            this.updateStatus('Chart updated successfully');

        } catch (error) {
            console.error('Chart update failed:', error);
            this.updateStatus(`Chart update failed: ${error.message}`);
        }
    }

    createArrowTraces(traces, metricNames, startHeight, endHeight, showMA, maWindow) {
        // Get data from Arrow files
        for (const [datasetName, {table, dataset}] of this.arrowData) {
            // Get height column
            const heightColumn = table.getChild('height');
            const heights = heightColumn.toArray();

            // Filter data by height range
            const indices = [];
            for (let i = 0; i < heights.length; i++) {
                const height = heights[i];
                if (height >= startHeight && height <= endHeight) {
                    indices.push(i);
                }
            }

            // Process each requested metric
            for (const metricName of metricNames) {
                if (table.schema.fields.find(f => f.name === metricName)) {
                    const metric = Array.from(this.selectedMetrics).find(m => m.name === metricName);
                    const column = table.getChild(metricName);

                    // Extract filtered data
                    const filteredHeights = indices.map(i => heights[i]);
                    const filteredValues = indices.map(i => column.get(i));

                    // Create trace
                    const yaxis = this.assignYAxis(metric.unit);
                    const trace = {
                        x: filteredHeights,
                        y: filteredValues,
                        name: `${metricName} (${metric.unit})`,
                        type: 'scatter',
                        mode: 'lines',
                        yaxis: yaxis,
                        line: {
                            width: 1.5
                        }
                    };

                    traces.push(trace);

                    // Add moving average if requested
                    if (showMA && filteredHeights.length > maWindow) {
                        const maData = this.calculateMovingAverage(filteredValues, maWindow);
                        const maTrace = {
                            x: filteredHeights.slice(maWindow - 1),
                            y: maData,
                            name: `${metricName} MA(${maWindow})`,
                            type: 'scatter',
                            mode: 'lines',
                            yaxis: yaxis,
                            line: {
                                width: 2,
                                dash: 'dash'
                            },
                            opacity: 0.8
                        };
                        traces.push(maTrace);
                    }
                }
            }
        }
    }

    createMockTraces(traces, metricNames, startHeight, endHeight, showMA, maWindow) {
        // Generate mock data for demonstration
        const numPoints = Math.min(1000, endHeight - startHeight + 1);
        const heights = Array.from({length: numPoints}, (_, i) =>
            startHeight + Math.floor(i * (endHeight - startHeight) / (numPoints - 1))
        );

        metricNames.forEach((metricName, index) => {
            const metric = Array.from(this.selectedMetrics).find(m => m.name === metricName);
            const yaxis = this.assignYAxis(metric.unit);

            // Generate realistic mock data based on metric type
            const values = heights.map(h => this.generateMockValue(metricName, h));

            traces.push({
                x: heights,
                y: values,
                type: 'scatter',
                mode: 'lines',
                name: `${metricName} (${metric.unit})`,
                yaxis: yaxis,
                line: { width: 2 },
                hovertemplate: `<b>${metricName}</b><br>Block: %{x}<br>Value: %{y:.2f} ${metric.unit}<extra></extra>`
            });

            // Add moving average if requested
            if (showMA && values.length >= maWindow) {
                const maValues = this.calculateMovingAverage(values, maWindow);
                const maHeights = heights.slice(maWindow - 1);

                traces.push({
                    x: maHeights,
                    y: maValues,
                    type: 'scatter',
                    mode: 'lines',
                    name: `${maWindow}MA ${metricName}`,
                    yaxis: yaxis,
                    line: { width: 3, dash: 'dash' },
                    opacity: 0.7
                });
            }
        });
    }

    generateMockValue(metricName, height) {
        // Generate realistic mock data patterns
        const baseHeight = 700000;
        const progress = (height - baseHeight) / 100000;

        switch (metricName) {
            case 'tx_count':
                return 1500 + Math.sin(progress * 10) * 500 + Math.random() * 300;
            case 'fee_avg':
                return 10 + Math.sin(progress * 5) * 8 + Math.random() * 5;
            case 'fee_min':
                return 1 + Math.random() * 2;
            case 'fee_max':
                return 100 + Math.sin(progress * 3) * 200 + Math.random() * 100;
            case 'block_size':
                return 1000000 + Math.sin(progress * 7) * 300000 + Math.random() * 100000;
            case 'sub_1sat_count':
                return Math.floor(Math.random() * 50);
            case 'op_return_max_size':
                return 40 + Math.floor(Math.random() * 40);
            case 'difficulty':
                return 20000000000000 + progress * 10000000000000 + Math.random() * 1000000000000;
            default:
                return Math.random() * 100;
        }
    }

    assignYAxis(unit) {
        // Assign metrics to left or right y-axis based on units
        const leftAxisUnits = ['transactions', 'sat/vB', 'bytes'];
        const rightAxisUnits = ['difficulty', 'EH/s'];

        if (leftAxisUnits.includes(unit)) {
            return 'y';
        } else if (rightAxisUnits.includes(unit)) {
            return 'y2';
        } else {
            // Default assignment based on typical ranges
            return 'y';
        }
    }

    calculateMovingAverage(values, window) {
        const ma = [];
        for (let i = window - 1; i < values.length; i++) {
            const sum = values.slice(i - window + 1, i + 1).reduce((a, b) => a + b, 0);
            ma.push(sum / window);
        }
        return ma;
    }

    getChartLayout() {
        const logScale = document.getElementById('logScale').checked;

        return {
            title: 'BlockStats Explorer',
            height: 600,
            xaxis: {
                title: 'Block Height',
                type: 'linear',
                showgrid: true
            },
            yaxis: {
                title: 'Left Axis',
                side: 'left',
                type: logScale ? 'log' : 'linear',
                showgrid: true
            },
            yaxis2: {
                title: 'Right Axis',
                side: 'right',
                overlaying: 'y',
                type: logScale ? 'log' : 'linear'
            },
            legend: {
                x: 0.02,
                y: 0.98,
                bgcolor: 'rgba(255,255,255,0.8)',
                bordercolor: 'rgba(0,0,0,0.2)',
                borderwidth: 1
            },
            hovermode: 'x unified',
            dragmode: 'zoom'
        };
    }

    updateChartLayout() {
        if (this.chart && this.chart.data && this.chart.data.length > 0) {
            Plotly.relayout(this.chart, this.getChartLayout());
        }
    }

    resetZoom() {
        if (this.chart) {
            Plotly.relayout(this.chart, {
                'xaxis.autorange': true,
                'yaxis.autorange': true,
                'yaxis2.autorange': true
            });
        }
    }

    updateStatus(message) {
        document.getElementById('loadingStatus').textContent = message;
    }

    updateDataStatus(message) {
        document.getElementById('dataStatus').textContent = message;
    }
}

// Initialize the application
const app = new BitcoinFeeExplorer();
app.initialize();