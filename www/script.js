import init, { FeeExplorer } from './pkg/fee_explorer.js';

class BitcoinFeeExplorer {
    constructor() {
        this.wasmModule = null;
        this.explorer = null;
        this.metadata = null;
        this.arrowData = new Map();
        this.selectedMetrics = new Set();
        this.chart = null;
    }

    async initialize() {
        try {
            this.updateStatus('Loading WASM module...');
            console.log('Starting WASM initialization...');

            // Initialize WASM
            console.log('Calling init()...');
            await init();
            console.log('WASM init() completed successfully');

            console.log('Creating FeeExplorer instance...');
            this.explorer = new FeeExplorer();
            console.log('FeeExplorer created successfully');

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
        const totalDatasets = this.metadata.datasets.length;
        this.updateProgress(0, true); // Show progress bar at 0%

        for (let i = 0; i < this.metadata.datasets.length; i++) {
            const dataset = this.metadata.datasets[i];
            try {
                // Update status and progress for current dataset
                this.updateStatus(`Loading dataset ${i + 1} of ${totalDatasets}: ${dataset.name}`);
                console.log(`Loading ${dataset.file}...`);

                // Step 1: Initiate download
                const baseProgress = (i / totalDatasets) * 100;
                const progressPerDataset = 100 / totalDatasets;
                this.updateProgress(baseProgress);

                const response = await fetch(`./data/${dataset.file}`);
                console.log(`Response status for ${dataset.file}:`, response.status, response.statusText);

                if (!response.ok) {
                    throw new Error(`Failed to load ${dataset.file}: ${response.status} ${response.statusText}`);
                }

                // Step 2: Download with progress tracking
                const contentLength = response.headers.get('Content-Length');
                const totalBytes = contentLength ? parseInt(contentLength) : null;
                console.log(`${dataset.file} size: ${totalBytes ? (totalBytes / 1024 / 1024).toFixed(1) + ' MB' : 'unknown size'}`);

                let downloadedBytes = 0;
                const chunks = [];
                const reader = response.body.getReader();

                while (true) {
                    const { done, value } = await reader.read();
                    if (done) break;

                    chunks.push(value);
                    downloadedBytes += value.length;

                    // Update progress during download (use 80% of the dataset's progress allocation for download)
                    if (totalBytes) {
                        const downloadProgress = (downloadedBytes / totalBytes) * 0.8; // 80% of this dataset's progress
                        this.updateProgress(baseProgress + (downloadProgress * progressPerDataset));
                        this.updateStatus(`Downloading ${dataset.name}: ${(downloadedBytes / 1024 / 1024).toFixed(1)} / ${(totalBytes / 1024 / 1024).toFixed(1)} MB`);
                    } else {
                        this.updateStatus(`Downloading ${dataset.name}: ${(downloadedBytes / 1024 / 1024).toFixed(1)} MB`);
                    }
                }

                // Combine chunks into ArrayBuffer
                const arrayBuffer = new Uint8Array(downloadedBytes);
                let offset = 0;
                for (const chunk of chunks) {
                    arrayBuffer.set(chunk, offset);
                    offset += chunk.length;
                }

                console.log(`${dataset.file} downloaded: ${arrayBuffer.byteLength} bytes`);

                // Step 3: Parse Arrow data (use next 15% of progress)
                this.updateProgress(baseProgress + (0.8 * progressPerDataset));
                this.updateStatus(`Parsing ${dataset.name}...`);

                // Try different Arrow API methods depending on version
                let table;
                try {
                    console.log(`Attempting to parse ${dataset.file} with tableFromIPC...`);
                    // Try the newer API first
                    table = Arrow.tableFromIPC(arrayBuffer.buffer);
                    console.log(`Successfully parsed ${dataset.file} with tableFromIPC`);
                } catch (e1) {
                    console.log(`tableFromIPC failed for ${dataset.file}:`, e1.message);
                    try {
                        console.log(`Attempting ${dataset.file} with Table.from...`);
                        // Try alternative API
                        table = Arrow.Table.from([Arrow.RecordBatch.from(arrayBuffer.buffer)]);
                        console.log(`Successfully parsed ${dataset.file} with Table.from`);
                    } catch (e2) {
                        console.log(`Table.from failed for ${dataset.file}:`, e2.message);
                        try {
                            console.log(`Attempting ${dataset.file} with RecordBatchFileReader...`);
                            // Try reading as IPC file
                            const reader = Arrow.RecordBatchFileReader.from(arrayBuffer.buffer);
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

                // Step 4: Process height data (use final 5% of progress)
                this.updateProgress(baseProgress + (0.95 * progressPerDataset));
                this.updateStatus(`Processing ${dataset.name}...`);

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

                // Complete this dataset
                this.updateProgress(((i + 1) / totalDatasets) * 100);

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

            // Update the data range display
            this.updateDataRangeDisplay(maxHeight);
        } else {
            console.warn('No height data found in Arrow files, keeping default range');
            document.getElementById('dataRange').textContent = 'No data available';
        }

        // Now load the updated metadata into WASM
        console.log('Loading metadata into WASM...');
        const updatedMetadata = JSON.stringify(this.metadata);
        console.log('Metadata to load:', updatedMetadata);
        await this.explorer.load_metadata(updatedMetadata);
        console.log('Metadata loaded into WASM successfully');

        // Hide progress bar and show completion
        this.updateProgress(100, false);
        this.updateDataStatus(`Loaded ${this.arrowData.size} Arrow datasets successfully`);
    }

    setupUI() {
        this.populateMetricSelect();
        this.setupEventListeners();
        this.initializeChart();

        // Hide loading headers after UI is fully set up with animation
        this.hideLoadingHeaders();
    }

    hideLoadingHeaders() {
        // Pause for 1 second to let users see the final message, then animate
        setTimeout(() => {
            const headerMobile = document.getElementById('headerOverlayMobile');
            const headerDesktop = document.getElementById('headerDesktop');

            if (headerMobile) {
                headerMobile.classList.add('slide-up');
                // Remove from DOM after animation completes
                setTimeout(() => {
                    headerMobile.style.display = 'none';
                }, 600);
            }

            if (headerDesktop) {
                headerDesktop.classList.add('slide-up');
                // Remove from DOM after animation completes
                setTimeout(() => {
                    headerDesktop.style.display = 'none';
                }, 600);
            }
        }, 1000); // 1 second pause before animation starts
    }

    populateMetricSelect() {
        console.log('Populating metric select...');
        const selectMobile = document.getElementById('metricSelectMobile');
        const selectDesktop = document.getElementById('metricSelectDesktop');

        if (selectMobile) selectMobile.innerHTML = '';
        if (selectDesktop) selectDesktop.innerHTML = '';

        try {
            // Get available metrics from WASM module
            console.log('Calling get_available_metrics()...');
            const metrics = this.explorer.get_available_metrics();
            console.log('Got metrics from WASM:', metrics);
            console.log('Number of metrics:', metrics ? metrics.length : 'null/undefined');

            if (!metrics || metrics.length === 0) {
                console.warn('No metrics returned from WASM module');
                if (selectMobile) selectMobile.innerHTML = '<option>No metrics available</option>';
                if (selectDesktop) selectDesktop.innerHTML = '<option>No metrics available</option>';
                return;
            }

            // Sort metrics alphabetically by name
            const sortedMetrics = [...metrics].sort((a, b) => a.name.localeCompare(b.name));

            sortedMetrics.forEach((metric, index) => {
                console.log(`Processing metric ${index}:`, metric);

                // Create option for mobile
                if (selectMobile) {
                    const optionMobile = document.createElement('option');
                    optionMobile.value = metric.name;
                    optionMobile.textContent = `${metric.name} (${metric.unit}) - ${metric.description}`;
                    optionMobile.dataset.unit = metric.unit;
                    optionMobile.dataset.dataset = metric.dataset;
                    selectMobile.appendChild(optionMobile);
                }

                // Create option for desktop
                if (selectDesktop) {
                    const optionDesktop = document.createElement('option');
                    optionDesktop.value = metric.name;
                    optionDesktop.textContent = `${metric.name} (${metric.unit}) - ${metric.description}`;
                    optionDesktop.dataset.unit = metric.unit;
                    optionDesktop.dataset.dataset = metric.dataset;
                    selectDesktop.appendChild(optionDesktop);
                }
            });

            console.log('Metric select populated successfully');
        } catch (error) {
            console.error('Error populating metrics:', error);
            if (selectMobile) selectMobile.innerHTML = '<option>Error loading metrics</option>';
            if (selectDesktop) selectDesktop.innerHTML = '<option>Error loading metrics</option>';
        }
    }

    setupEventListeners() {
        const metricSelectMobile = document.getElementById('metricSelectMobile');
        const metricSelectDesktop = document.getElementById('metricSelectDesktop');
        const updateButtonMobile = document.getElementById('updateChartMobile');
        const updateButtonDesktop = document.getElementById('updateChartDesktop');
        const resetZoomButtonMobile = document.getElementById('resetZoomMobile');
        const resetZoomButtonDesktop = document.getElementById('resetZoomDesktop');
        const logScaleCheckboxMobile = document.getElementById('logScaleMobile');
        const logScaleCheckboxDesktop = document.getElementById('logScaleDesktop');
        const showMACheckboxMobile = document.getElementById('showMAMobile');
        const showMACheckboxDesktop = document.getElementById('showMADesktop');

        // Mobile collapsible sections
        this.setupMobileControls();

        // Handle metric selection for both mobile and desktop
        const handleMetricChange = (e) => {
            this.selectedMetrics.clear();
            Array.from(e.target.selectedOptions).forEach(option => {
                this.selectedMetrics.add({
                    name: option.value,
                    unit: option.dataset.unit,
                    dataset: option.dataset.dataset
                });
            });
        };

        if (metricSelectMobile) {
            metricSelectMobile.addEventListener('change', handleMetricChange);
        }
        if (metricSelectDesktop) {
            metricSelectDesktop.addEventListener('change', handleMetricChange);
        }

        if (updateButtonMobile) {
            updateButtonMobile.addEventListener('click', () => this.updateChart());
        }
        if (updateButtonDesktop) {
            updateButtonDesktop.addEventListener('click', () => this.updateChart());
        }
        if (resetZoomButtonMobile) {
            resetZoomButtonMobile.addEventListener('click', () => this.resetZoom());
        }
        if (resetZoomButtonDesktop) {
            resetZoomButtonDesktop.addEventListener('click', () => this.resetZoom());
        }

        if (logScaleCheckboxMobile) {
            logScaleCheckboxMobile.addEventListener('change', () => this.updateChartLayout());
        }
        if (logScaleCheckboxDesktop) {
            logScaleCheckboxDesktop.addEventListener('change', () => this.updateChartLayout());
        }
        if (showMACheckboxMobile) {
            showMACheckboxMobile.addEventListener('change', () => this.updateChart());
        }
        if (showMACheckboxDesktop) {
            showMACheckboxDesktop.addEventListener('change', () => this.updateChart());
        }

        // Enable multi-select with Ctrl/Cmd for both mobile and desktop
        const handleMultiSelect = (e) => {
            if (e.ctrlKey || e.metaKey) {
                e.preventDefault();
                const option = e.target;
                if (option.tagName === 'OPTION') {
                    option.selected = !option.selected;
                    e.currentTarget.dispatchEvent(new Event('change'));
                }
            }
        };

        if (metricSelectMobile) {
            metricSelectMobile.addEventListener('mousedown', handleMultiSelect);
        }
        if (metricSelectDesktop) {
            metricSelectDesktop.addEventListener('mousedown', handleMultiSelect);
        }

        // Handle window resize for responsive chart height
        window.addEventListener('resize', () => {
            if (this.chart && this.chart.data && this.chart.data.length > 0) {
                this.updateChartLayout();
            }
        });
    }

    initializeChart() {
        const chartDiv = document.getElementById('mainChart');

        // Responsive height: full screen on mobile, fixed on desktop
        const isMobile = window.innerWidth <= 767;
        const chartHeight = isMobile ? window.innerHeight : 600;

        const layout = {
            title: isMobile ? null : 'BlockStats Explorer', // Hide title on mobile for more space
            height: chartHeight,
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
            dragmode: 'zoom' // Try zoom mode for both mobile and desktop
        };

        const config = isMobile ? {
            // Clean mobile configuration
            responsive: true,
            displayModeBar: false,  // Hide mode bar for clean mobile interface
            scrollZoom: true,
            doubleClick: 'reset+autosize'
        } : {
            // Desktop configuration
            responsive: true,
            displayModeBar: true,
            modeBarButtonsToAdd: ['pan2d', 'zoom2d', 'zoomIn2d', 'zoomOut2d', 'resetScale2d'],
            scrollZoom: true,
            doubleClick: 'reset+autosize'
        };

        Plotly.newPlot(chartDiv, [], layout, config);
        this.chart = chartDiv;

        // Remove debugging for now and try a simpler approach
        if (isMobile) {
            console.log('Mobile chart initialized with config:', config);

            // Maybe the issue is with Plotly.js touch handling
            // Let's try disabling all custom touch handling and let Plotly handle it
            console.log('Mobile chart ready for touch interactions');
        }
    }

    async updateChart() {
        if (this.selectedMetrics.size === 0) {
            alert('Please select at least one metric');
            return;
        }

        this.updateStatus('Updating chart...');

        try {
            const startHeight = this.metadata.block_range.start;
            const endHeight = this.metadata.block_range.end;

            // Check both mobile and desktop checkboxes
            const showMAMobile = document.getElementById('showMAMobile');
            const showMADesktop = document.getElementById('showMADesktop');
            const showMA = (showMAMobile && showMAMobile.checked) || (showMADesktop && showMADesktop.checked);

            const maWindowMobile = document.getElementById('maWindowMobile');
            const maWindowDesktop = document.getElementById('maWindowDesktop');
            const maWindow = parseInt((maWindowMobile && maWindowMobile.value) || (maWindowDesktop && maWindowDesktop.value)) || 200;

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
        const logScaleMobile = document.getElementById('logScaleMobile');
        const logScaleDesktop = document.getElementById('logScaleDesktop');
        const logScale = (logScaleMobile && logScaleMobile.checked) || (logScaleDesktop && logScaleDesktop.checked);

        // Responsive height: full screen on mobile, fixed on desktop
        const isMobile = window.innerWidth <= 767;
        const chartHeight = isMobile ? window.innerHeight : 600;

        return {
            title: isMobile ? null : 'BlockStats Explorer', // Hide title on mobile for more space
            height: chartHeight,
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
            dragmode: 'zoom' // Try zoom mode for both mobile and desktop
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
        const loadingStatusMobile = document.getElementById('loadingStatusMobile');
        const loadingStatusDesktop = document.getElementById('loadingStatusDesktop');

        if (loadingStatusMobile) loadingStatusMobile.textContent = message;
        if (loadingStatusDesktop) loadingStatusDesktop.textContent = message;
    }

    updateProgress(percentage, show = true) {
        const progressContainerMobile = document.getElementById('progressContainerMobile');
        const progressContainerDesktop = document.getElementById('progressContainerDesktop');
        const progressBarMobile = document.getElementById('progressBarMobile');
        const progressBarDesktop = document.getElementById('progressBarDesktop');

        if (show) {
            if (progressContainerMobile) progressContainerMobile.style.display = 'block';
            if (progressContainerDesktop) progressContainerDesktop.style.display = 'block';
        } else {
            if (progressContainerMobile) progressContainerMobile.style.display = 'none';
            if (progressContainerDesktop) progressContainerDesktop.style.display = 'none';
        }

        if (progressBarMobile) progressBarMobile.style.width = `${percentage}%`;
        if (progressBarDesktop) progressBarDesktop.style.width = `${percentage}%`;
    }

    updateDataStatus(message) {
        const dataStatusMobile = document.getElementById('dataStatusMobile');
        const dataStatusDesktop = document.getElementById('dataStatusDesktop');
        if (dataStatusMobile) dataStatusMobile.textContent = message;
        if (dataStatusDesktop) dataStatusDesktop.textContent = message;
    }

    updateDataRangeDisplay(maxHeight) {
        // Find the timestamp for the highest block height
        let maxTimestamp = null;

        for (const [datasetName, {table, dataset}] of this.arrowData) {
            const heightColumn = table.getChild('height');
            const timestampColumn = table.getChild('timestamp');

            if (heightColumn && timestampColumn) {
                const heights = heightColumn.toArray();
                const timestamps = timestampColumn.toArray();

                for (let i = 0; i < heights.length; i++) {
                    if (heights[i] === maxHeight) {
                        maxTimestamp = timestamps[i];
                        break;
                    }
                }

                if (maxTimestamp !== null) break;
            }
        }

        // Format the display message
        let message = `Latest block: ${maxHeight}`;
        if (maxTimestamp !== null) {
            const date = new Date(maxTimestamp * 1000); // Convert Unix timestamp to Date
            message += ` (${date.toLocaleDateString()} ${date.toLocaleTimeString()})`;
        }

        document.getElementById('dataRange').textContent = message;
    }

    setupMobileControls() {
        // Handle collapsible control sections
        const controlHeaders = document.querySelectorAll('.control-header');
        controlHeaders.forEach(header => {
            header.addEventListener('click', () => {
                const section = header.parentElement;
                section.classList.toggle('collapsed');
            });
        });

        // Auto-collapse sections after initial load
        setTimeout(() => {
            const sections = document.querySelectorAll('.control-section');
            sections.forEach(section => {
                section.classList.add('collapsed');
            });
        }, 2000);
    }
}

// Initialize the application
const app = new BitcoinFeeExplorer();
app.initialize();